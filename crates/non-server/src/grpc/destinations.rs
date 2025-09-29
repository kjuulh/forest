use anyhow::Context;
use non_grpc_interface::{destination_service_server::DestinationService, *};
use tonic::Response;

use crate::{
    grpc::artifacts::GrpcErrorExt, services::destination_registry::DestinationRegistryState,
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

        self.state
            .destination_registry()
            .create_destination(&req.name)
            .await
            .context("create destination")
            .to_internal_error()?;

        Ok(Response::new(CreateDestinationResponse {}))
    }
}
