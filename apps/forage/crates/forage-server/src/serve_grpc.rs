use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use forage_core::compute::ComputeScheduler;
use forage_grpc::forage_service_server::ForageServiceServer;
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::compute_grpc::ForageServiceImpl;

pub struct ServeGrpc {
    pub addr: SocketAddr,
    pub scheduler: Arc<dyn ComputeScheduler>,
}

impl Component for ServeGrpc {
    fn info(&self) -> ComponentInfo {
        "forage/grpc".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let svc = ForageServiceImpl {
            scheduler: self.scheduler.clone(),
        };

        tracing::info!("gRPC server listening on {}", self.addr);

        tonic::transport::Server::builder()
            .add_service(ForageServiceServer::new(svc))
            .serve_with_shutdown(self.addr, async move {
                cancellation_token.cancelled().await;
            })
            .await
            .context("failed to run gRPC server")?;

        Ok(())
    }
}
