use non_grpc_interface::{namespace_service_server::NamespaceService, *};

use crate::{services::namespaces::NamespaceServiceState, state::State};

pub struct NamespacesServer {
    pub state: State,
}

#[async_trait::async_trait]
impl NamespaceService for NamespacesServer {
    async fn create(
        &self,
        request: tonic::Request<CreateRequest>,
    ) -> std::result::Result<tonic::Response<CreateResponse>, tonic::Status> {
        let inner = request.into_inner();

        self.state
            .namespace_service()
            .create_namespace(&inner.namespace)
            .await
            .inspect_err(|e| tracing::warn!("failed to create namespace: {:#?}", e))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(CreateResponse {}))
    }
}
