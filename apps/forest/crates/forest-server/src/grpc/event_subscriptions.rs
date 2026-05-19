use anyhow::Context;
use forest_grpc_interface::{event_subscription_service_server::EventSubscriptionService, *};
use tonic::Response;

use crate::{
    actor::Actor,
    grpc::{artifacts::GrpcErrorExt, authorize},
    services::event_subscription::{
        CreateSubscriptionParams, EventSubscriptionRegistryState, SubscriptionRecord,
    },
    state::State,
};

pub struct EventSubscriptionsServer {
    pub state: State,
}

fn record_to_grpc(r: SubscriptionRecord) -> EventSubscription {
    EventSubscription {
        id: r.id.to_string(),
        organisation: r.organisation,
        name: r.name,
        resource_types: r.resource_types,
        actions: r.actions,
        projects: r.projects,
        status: r.status,
        cursor: r.cursor,
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
    }
}

#[async_trait::async_trait]
impl EventSubscriptionService for EventSubscriptionsServer {
    async fn create_event_subscription(
        &self,
        request: tonic::Request<CreateEventSubscriptionRequest>,
    ) -> Result<Response<CreateEventSubscriptionResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        let (app_id, user_id) = match &actor {
            Actor::App {
                app_id,
                organisation_id: _,
            } => (Some(*app_id), None),
            Actor::User { user_id } => (None, Some(*user_id)),
            Actor::ServiceAccount { service_account_id } => {
                // Service accounts are tracked via their id in the app_id column
                (Some(*service_account_id), None)
            }
        };

        let rec = self
            .state
            .event_subscription_registry()
            .create(CreateSubscriptionParams {
                organisation: req.organisation,
                name: req.name,
                resource_types: req.resource_types,
                actions: req.actions,
                projects: req.projects,
                created_by_app_id: app_id,
                created_by_user_id: user_id,
            })
            .await
            .context("create event subscription")
            .to_internal_error()?;

        Ok(Response::new(CreateEventSubscriptionResponse {
            subscription: Some(record_to_grpc(rec)),
        }))
    }

    async fn update_event_subscription(
        &self,
        request: tonic::Request<UpdateEventSubscriptionRequest>,
    ) -> Result<Response<UpdateEventSubscriptionResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        let rec = self
            .state
            .event_subscription_registry()
            .update(
                &req.organisation,
                &req.name,
                req.status.as_deref(),
                req.update_filters,
                req.resource_types,
                req.actions,
                req.projects,
            )
            .await
            .context("update event subscription")
            .to_internal_error()?;

        Ok(Response::new(UpdateEventSubscriptionResponse {
            subscription: Some(record_to_grpc(rec)),
        }))
    }

    async fn delete_event_subscription(
        &self,
        request: tonic::Request<DeleteEventSubscriptionRequest>,
    ) -> Result<Response<DeleteEventSubscriptionResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        self.state
            .event_subscription_registry()
            .delete(&req.organisation, &req.name)
            .await
            .context("delete event subscription")
            .to_internal_error()?;

        Ok(Response::new(DeleteEventSubscriptionResponse {}))
    }

    async fn list_event_subscriptions(
        &self,
        request: tonic::Request<ListEventSubscriptionsRequest>,
    ) -> Result<Response<ListEventSubscriptionsResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        let recs = self
            .state
            .event_subscription_registry()
            .list(&req.organisation)
            .await
            .context("list event subscriptions")
            .to_internal_error()?;

        Ok(Response::new(ListEventSubscriptionsResponse {
            subscriptions: recs.into_iter().map(record_to_grpc).collect(),
        }))
    }
}
