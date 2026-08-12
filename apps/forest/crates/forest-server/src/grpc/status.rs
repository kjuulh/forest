use forest_grpc_interface::{status_service_server::StatusService, *};

use crate::state::State;

pub struct StatusServer {
    pub state: State,
}

#[async_trait::async_trait]
impl StatusService for StatusServer {
    /// Liveness plus build provenance.
    ///
    /// Deliberately unauthenticated, like the liveness check it replaces: it
    /// is the probe endpoint, and gating it would break the health check. The
    /// fields it adds are the server's own version and commit, which the
    /// deployed image already advertises.
    async fn status(
        &self,
        _request: tonic::Request<GetStatusRequest>,
    ) -> std::result::Result<tonic::Response<GetStatusResponse>, tonic::Status> {
        let info = crate::build_info::BuildInfo::from_env();
        Ok(tonic::Response::new(GetStatusResponse {
            version: info.version,
            commit: info.commit,
            build_time: info.build_time,
        }))
    }
}
