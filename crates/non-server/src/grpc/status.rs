use non_grpc_interface::{status_service_server::StatusService, *};

use crate::state::State;

pub struct StatusServer {
    pub state: State,
}

#[async_trait::async_trait]
impl StatusService for StatusServer {
    async fn status(
        &self,
        _request: tonic::Request<GetStatusRequest>,
    ) -> std::result::Result<tonic::Response<GetStatusResponse>, tonic::Status> {
        Ok(tonic::Response::new(GetStatusResponse {}))
    }
}
