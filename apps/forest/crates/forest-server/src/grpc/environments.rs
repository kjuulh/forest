use anyhow::Context;
use forest_grpc_interface::{environment_service_server::EnvironmentService, *};
use tonic::Response;

use crate::{
    grpc::{artifacts::GrpcErrorExt, authorize},
    services::{
        environment_registry::{EnvironmentRecord, EnvironmentRegistryState},
        event_bus::{EventBusState, EventPayload},
    },
    state::State,
};

pub struct EnvironmentsServer {
    pub state: State,
}

fn record_to_grpc(r: EnvironmentRecord) -> Environment {
    Environment {
        id: r.id.to_string(),
        organisation: r.organisation,
        name: r.name,
        description: r.description,
        sort_order: r.sort_order,
        created_at: r.created_at.to_rfc3339(),
    }
}

#[async_trait::async_trait]
impl EnvironmentService for EnvironmentsServer {
    async fn create_environment(
        &self,
        request: tonic::Request<CreateEnvironmentRequest>,
    ) -> Result<Response<CreateEnvironmentResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        let _authz = authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        let rec = self
            .state
            .environment_registry()
            .create(
                &req.organisation,
                &req.name,
                req.description.as_deref(),
                req.sort_order,
            )
            .await
            .context("create environment")
            .to_internal_error()?;

        self.state.event_bus().emit(EventPayload {
            organisation: req.organisation.clone(),
            project: String::new(),
            resource_type: "environment",
            action: "created",
            resource_id: rec.id.to_string(),
            metadata: [("name".into(), req.name.clone())].into(),
        }).await;

        Ok(Response::new(CreateEnvironmentResponse {
            environment: Some(record_to_grpc(rec)),
        }))
    }

    async fn get_environment(
        &self,
        request: tonic::Request<GetEnvironmentRequest>,
    ) -> Result<Response<GetEnvironmentResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let identifier = req
            .identifier
            .context("identifier is required")
            .to_internal_error()?;

        let rec = match identifier {
            get_environment_request::Identifier::Id(id) => {
                let id: uuid::Uuid = id.parse().context("invalid id").to_internal_error()?;
                self.state
                    .environment_registry()
                    .get_by_id(&id)
                    .await
                    .context("get environment")
                    .to_internal_error()?
            }
            get_environment_request::Identifier::Lookup(lookup) => self
                .state
                .environment_registry()
                .get_by_org_name(&lookup.organisation, &lookup.name)
                .await
                .context("get environment")
                .to_internal_error()?,
        };

        let rec = rec
            .context("environment not found")
            .to_internal_error()?;

        let _authz = authorize::require_org_access(
            &self.state.db,
            &actor,
            &rec.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        Ok(Response::new(GetEnvironmentResponse {
            environment: Some(record_to_grpc(rec)),
        }))
    }

    async fn list_environments(
        &self,
        request: tonic::Request<ListEnvironmentsRequest>,
    ) -> Result<Response<ListEnvironmentsResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        let _authz = authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        let recs = self
            .state
            .environment_registry()
            .list(&req.organisation)
            .await
            .context("list environments")
            .to_internal_error()?;

        Ok(Response::new(ListEnvironmentsResponse {
            environments: recs.into_iter().map(record_to_grpc).collect(),
        }))
    }

    async fn update_environment(
        &self,
        request: tonic::Request<UpdateEnvironmentRequest>,
    ) -> Result<Response<UpdateEnvironmentResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        let id: uuid::Uuid = req
            .id
            .parse()
            .context("invalid id")
            .to_internal_error()?;
        let org_name = sqlx::query_scalar!(
            "SELECT organisation FROM environments WHERE id = $1",
            id
        )
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| {
            tracing::error!("authz: {e}");
            tonic::Status::internal("lookup failed")
        })?
        .ok_or_else(|| tonic::Status::not_found("environment not found"))?;
        let _authz = authorize::require_org_access(
            &self.state.db,
            &actor,
            &org_name,
            authorize::OrgRole::Member,
        )
        .await?;

        let rec = self
            .state
            .environment_registry()
            .update(&id, req.description.as_deref(), req.sort_order)
            .await
            .context("update environment")
            .to_internal_error()?;

        self.state.event_bus().emit(EventPayload {
            organisation: rec.organisation.clone(),
            project: String::new(),
            resource_type: "environment",
            action: "updated",
            resource_id: id.to_string(),
            metadata: [("name".into(), rec.name.clone())].into(),
        }).await;

        Ok(Response::new(UpdateEnvironmentResponse {
            environment: Some(record_to_grpc(rec)),
        }))
    }

    async fn delete_environment(
        &self,
        request: tonic::Request<DeleteEnvironmentRequest>,
    ) -> Result<Response<DeleteEnvironmentResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        let id: uuid::Uuid = req
            .id
            .parse()
            .context("invalid id")
            .to_internal_error()?;
        let org_name = sqlx::query_scalar!(
            "SELECT organisation FROM environments WHERE id = $1",
            id
        )
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| {
            tracing::error!("authz: {e}");
            tonic::Status::internal("lookup failed")
        })?
        .ok_or_else(|| tonic::Status::not_found("environment not found"))?;
        let _authz = authorize::require_org_access(
            &self.state.db,
            &actor,
            &org_name,
            authorize::OrgRole::Member,
        )
        .await?;

        self.state
            .environment_registry()
            .delete(&id)
            .await
            .context("delete environment")
            .to_internal_error()?;

        self.state.event_bus().emit(EventPayload {
            organisation: String::new(),
            project: String::new(),
            resource_type: "environment",
            action: "deleted",
            resource_id: id.to_string(),
            metadata: Default::default(),
        }).await;

        Ok(Response::new(DeleteEnvironmentResponse {}))
    }
}
