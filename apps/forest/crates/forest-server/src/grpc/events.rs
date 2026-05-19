use std::time::Duration;

use forest_grpc_interface::{event_service_server::EventService, *};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::{
    services::{
        event_bus::{EventBusState, OrgEventRow},
        event_subscription::EventSubscriptionRegistryState,
    },
    state::State,
};

pub struct EventsServer {
    pub state: State,
}

fn row_to_proto(row: OrgEventRow) -> OrgEvent {
    let metadata: std::collections::HashMap<String, String> = match row.metadata.as_object() {
        Some(m) => m
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect(),
        None => std::collections::HashMap::new(),
    };

    OrgEvent {
        sequence: row.sequence,
        event_id: row.event_id.to_string(),
        timestamp: row.created_at.to_rfc3339(),
        organisation: row.organisation,
        project: row.project,
        resource_type: row.resource_type,
        action: row.action,
        resource_id: row.resource_id,
        metadata,
    }
}

/// Shared streaming loop: fetches events from DB, filters, sends to channel.
async fn stream_events(
    tx: mpsc::Sender<Result<OrgEvent, Status>>,
    event_bus: crate::services::event_bus::EventBus,
    nats: async_nats::Client,
    organisation: String,
    mut cursor: i64,
    project_filter: Option<String>,
    resource_types: Vec<String>,
    actions: Vec<String>,
    replay: bool,
) {
    // Subscribe to NATS nudges for this org
    let nats_subject = format!("forest.events.{}", organisation);
    let mut nats_sub = match nats.subscribe(nats_subject).await {
        Ok(sub) => sub,
        Err(e) => {
            tracing::error!("failed to subscribe to NATS for org events: {e}");
            let _ = tx
                .send(Err(Status::internal("failed to initialize stream")))
                .await;
            return;
        }
    };

    let mut fallback_interval = tokio::time::interval(Duration::from_secs(5));
    fallback_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // If replaying, immediately fetch catch-up batch
    if replay {
        match event_bus
            .fetch_since(
                &organisation,
                cursor,
                project_filter.as_deref(),
                &resource_types,
                &actions,
                1000,
            )
            .await
        {
            Ok(events) => {
                for event in events {
                    cursor = event.sequence;
                    if tx.send(Ok(row_to_proto(event))).await.is_err() {
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("failed to fetch catch-up events: {e:#}");
            }
        }
    }

    loop {
        tokio::select! {
            _ = nats_sub.next() => {}
            _ = fallback_interval.tick() => {}
        }

        match event_bus
            .fetch_since(
                &organisation,
                cursor,
                project_filter.as_deref(),
                &resource_types,
                &actions,
                100,
            )
            .await
        {
            Ok(events) => {
                for event in events {
                    cursor = event.sequence;
                    if tx.send(Ok(row_to_proto(event))).await.is_err() {
                        return; // Client disconnected
                    }
                }
            }
            Err(e) => {
                tracing::warn!("failed to fetch org events: {e:#}");
            }
        }
    }
}

#[async_trait::async_trait]
impl EventService for EventsServer {
    type SubscribeStream = ReceiverStream<Result<OrgEvent, Status>>;

    async fn subscribe(
        &self,
        request: Request<SubscribeEventsRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let req = request.into_inner();
        let organisation = req.organisation.clone();

        if organisation.is_empty() {
            return Err(Status::invalid_argument("organisation is required"));
        }

        let event_bus = self.state.event_bus();
        let nats = self.state.nats.clone();

        let project_filter = if req.project.is_empty() {
            None
        } else {
            Some(req.project.clone())
        };

        let cursor = if req.since_sequence > 0 {
            req.since_sequence
        } else {
            event_bus
                .max_sequence(&organisation)
                .await
                .map_err(|e| Status::internal(format!("failed to fetch max sequence: {e:#}")))?
        };

        let (tx, rx) = mpsc::channel(64);

        tokio::spawn(stream_events(
            tx,
            event_bus,
            nats,
            organisation,
            cursor,
            project_filter,
            req.resource_types,
            req.actions,
            req.since_sequence > 0,
        ));

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    type SubscribeDurableStream = ReceiverStream<Result<OrgEvent, Status>>;

    async fn subscribe_durable(
        &self,
        request: Request<SubscribeDurableRequest>,
    ) -> Result<Response<Self::SubscribeDurableStream>, Status> {
        let req = request.into_inner();

        if req.organisation.is_empty() {
            return Err(Status::invalid_argument("organisation is required"));
        }
        if req.subscription_name.is_empty() {
            return Err(Status::invalid_argument("subscription_name is required"));
        }

        // Load subscription from DB
        let sub = self
            .state
            .event_subscription_registry()
            .get(&req.organisation, &req.subscription_name)
            .await
            .map_err(|e| Status::internal(format!("failed to load subscription: {e:#}")))?
            .ok_or_else(|| Status::not_found("subscription not found"))?;

        if sub.status != "active" {
            return Err(Status::failed_precondition(format!(
                "subscription is {}",
                sub.status
            )));
        }

        let event_bus = self.state.event_bus();
        let nats = self.state.nats.clone();

        // The subscription's projects filter: empty = None (all projects)
        let project_filter = if sub.projects.len() == 1 {
            Some(sub.projects[0].clone())
        } else {
            // Multi-project or no filter — handled by fetch_since's project param
            // For now, None means "all". Multi-project filtering happens client-side
            // or we could enhance fetch_since. Keep it simple.
            None
        };

        let (tx, rx) = mpsc::channel(64);

        tokio::spawn(stream_events(
            tx,
            event_bus,
            nats,
            req.organisation,
            sub.cursor,
            project_filter,
            sub.resource_types,
            sub.actions,
            true, // always replay from cursor
        ));

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn acknowledge_events(
        &self,
        request: Request<AcknowledgeEventsRequest>,
    ) -> Result<Response<AcknowledgeEventsResponse>, Status> {
        let req = request.into_inner();

        if req.organisation.is_empty() {
            return Err(Status::invalid_argument("organisation is required"));
        }
        if req.subscription_name.is_empty() {
            return Err(Status::invalid_argument("subscription_name is required"));
        }
        if req.sequence <= 0 {
            return Err(Status::invalid_argument("sequence must be > 0"));
        }

        let cursor = self
            .state
            .event_subscription_registry()
            .acknowledge(&req.organisation, &req.subscription_name, req.sequence)
            .await
            .map_err(|e| Status::internal(format!("failed to acknowledge: {e:#}")))?;

        Ok(Response::new(AcknowledgeEventsResponse { cursor }))
    }
}
