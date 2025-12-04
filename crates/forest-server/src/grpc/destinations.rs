use anyhow::Context;
use forest_grpc_interface::{destination_service_server::DestinationService, *};
use tonic::Response;

use crate::{
    destination_services::DestinationServicesState,
    grpc::artifacts::GrpcErrorExt,
    services::{
        destination_registry::DestinationRegistryState, release_registry::ReleaseRegistryState,
    },
    state::State,
};

pub struct DestinationServer {
    pub state: State,
}

#[async_trait::async_trait]
impl DestinationService for DestinationServer {
    async fn create_destination(
        &self,
        request: tonic::Request<CreateDestinationRequest>,
    ) -> std::result::Result<tonic::Response<CreateDestinationResponse>, tonic::Status> {
        let req = request.into_inner();

        let dest_type: forest_models::DestinationType = req
            .r#type
            .context("destination type is required")
            .to_internal_error()?
            .into();

        self.state
            .destination_services()
            .get_destination(&dest_type.organisation, &dest_type.name, dest_type.version)
            .context("failed to find destination implementation")
            .to_internal_error()?;

        self.state
            .destination_registry()
            .create_destination(&req.name, &req.environment, req.metadata, dest_type)
            .await
            .context("create destination")
            .to_internal_error()?;

        Ok(Response::new(CreateDestinationResponse {}))
    }

    async fn update_destination(
        &self,
        request: tonic::Request<UpdateDestinationRequest>,
    ) -> std::result::Result<tonic::Response<UpdateDestinationResponse>, tonic::Status> {
        let req = request.into_inner();

        self.state
            .destination_registry()
            .update_destination(&req.name, req.metadata)
            .await
            .context("update destination")
            .to_internal_error()?;

        Ok(Response::new(UpdateDestinationResponse {}))
    }

    async fn get_destinations(
        &self,
        request: tonic::Request<GetDestinationsRequest>,
    ) -> std::result::Result<tonic::Response<GetDestinationsResponse>, tonic::Status> {
        tracing::debug!("get destinations");
        let _req = request.into_inner();

        let destinations = self
            .state
            .release_registry()
            .get_destinations()
            .await
            .context("failed to find destinations")
            .to_internal_error()?;

        Ok(Response::new(GetDestinationsResponse {
            destinations: destinations.into_iter().map(|n| n.into()).collect(),
        }))
    }
}
