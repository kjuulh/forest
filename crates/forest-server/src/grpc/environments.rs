use anyhow::Context;
use forest_grpc_interface::{environment_service_server::EnvironmentService, *};
use tonic::Response;

use crate::{
    grpc::artifacts::GrpcErrorExt,
    services::environment_registry::{EnvironmentRecord, EnvironmentRegistryState},
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
        let req = request.into_inner();

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

        Ok(Response::new(CreateEnvironmentResponse {
            environment: Some(record_to_grpc(rec)),
        }))
    }

    async fn get_environment(
        &self,
        request: tonic::Request<GetEnvironmentRequest>,
    ) -> Result<Response<GetEnvironmentResponse>, tonic::Status> {
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

        Ok(Response::new(GetEnvironmentResponse {
            environment: Some(record_to_grpc(rec)),
        }))
    }

    async fn list_environments(
        &self,
        request: tonic::Request<ListEnvironmentsRequest>,
    ) -> Result<Response<ListEnvironmentsResponse>, tonic::Status> {
        let req = request.into_inner();

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
        let req = request.into_inner();
        let id: uuid::Uuid = req
            .id
            .parse()
            .context("invalid id")
            .to_internal_error()?;

        let rec = self
            .state
            .environment_registry()
            .update(&id, req.description.as_deref(), req.sort_order)
            .await
            .context("update environment")
            .to_internal_error()?;

        Ok(Response::new(UpdateEnvironmentResponse {
            environment: Some(record_to_grpc(rec)),
        }))
    }

    async fn delete_environment(
        &self,
        request: tonic::Request<DeleteEnvironmentRequest>,
    ) -> Result<Response<DeleteEnvironmentResponse>, tonic::Status> {
        let req = request.into_inner();
        let id: uuid::Uuid = req
            .id
            .parse()
            .context("invalid id")
            .to_internal_error()?;

        self.state
            .environment_registry()
            .delete(&id)
            .await
            .context("delete environment")
            .to_internal_error()?;

        Ok(Response::new(DeleteEnvironmentResponse {}))
    }
}
