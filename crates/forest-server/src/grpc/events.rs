use std::time::Duration;

use forest_grpc_interface::{event_service_server::EventService, *};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::{
    services::event_bus::{EventBusState, OrgEventRow},
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
        let resource_types = req.resource_types.clone();
        let actions = req.actions.clone();
        let since_sequence = req.since_sequence;

        let (tx, rx) = mpsc::channel(64);

        tokio::spawn(async move {
            // Determine starting cursor
            let mut cursor = if since_sequence > 0 {
                since_sequence
            } else {
                match event_bus.max_sequence(&organisation).await {
                    Ok(seq) => seq,
                    Err(e) => {
                        tracing::error!("failed to fetch max sequence: {e:#}");
                        let _ = tx
                            .send(Err(Status::internal("failed to initialize stream")))
                            .await;
                        return;
                    }
                }
            };

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

            // If replaying from a past sequence, immediately fetch catch-up batch
            if since_sequence > 0 {
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
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}
