use std::net::SocketAddr;

use namespaces::NamespacesServer;
use non_grpc_interface::{
    artifact_service_server::ArtifactServiceServer,
    destination_service_server::DestinationServiceServer,
    namespace_service_server::NamespaceServiceServer,
    registry_service_server::RegistryServiceServer, release_service_server::ReleaseServiceServer,
    status_service_server::StatusServiceServer,
};
use notmad::MadError;
use registry::RegistryServer;
use status::StatusServer;
use tokio_util::sync::CancellationToken;

use crate::{
    grpc::{artifacts::ArtifactServer, destinations::DestinationServer, release::ReleaseServer},
    state::State,
};

mod artifacts;
mod destinations;
mod namespaces;
mod registry;
mod release;
mod status;

pub struct GrpcServer {
    pub host: SocketAddr,
    pub state: State,
}

impl GrpcServer {
    pub async fn serve(&self, cancellation_token: CancellationToken) -> anyhow::Result<()> {
        tracing::info!("serving grpc on {}", self.host);

        tonic::transport::Server::builder()
            .add_service(StatusServiceServer::new(StatusServer {
                state: self.state.clone(),
            }))
            .add_service(RegistryServiceServer::new(RegistryServer {
                state: self.state.clone(),
            }))
            .add_service(NamespaceServiceServer::new(NamespacesServer {
                state: self.state.clone(),
            }))
            .add_service(ArtifactServiceServer::new(ArtifactServer {
                state: self.state.clone(),
            }))
            .add_service(ReleaseServiceServer::new(ReleaseServer {
                state: self.state.clone(),
            }))
            .add_service(DestinationServiceServer::new(DestinationServer {
                state: self.state.clone(),
            }))
            .serve_with_shutdown(
                self.host,
                async move { cancellation_token.cancelled().await },
            )
            .await?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl notmad::Component for GrpcServer {
    fn name(&self) -> Option<String> {
        Some("non-server/grpc".into())
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        self.serve(cancellation_token)
            .await
            .map_err(MadError::Inner)?;

        Ok(())
    }
}
