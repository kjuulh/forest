pub mod fluxv1;

use forest_grpc_interface::DestinationCapability;

use crate::backend::{DestinationBackend, DestinationConfig};

/// Trait for destination handlers that run inside a runner.
///
/// This is the runner-side equivalent of `DestinationEdge` in forest-server.
/// Implementations receive a `DestinationBackend` for data access and logging.
///
/// When `forest-server` uses this as a library for in-process execution,
/// it wraps this trait with an adapter that provides files from the DB
/// and logs via the existing `DestinationLogger`.
#[async_trait::async_trait]
pub trait RunnerDestination: Send + Sync {
    /// What destination types this handler supports.
    fn capabilities(&self) -> Vec<DestinationCapability>;

    /// Optional prepare/dry-run step.
    async fn prepare(&self, ctx: &RunnerContext) -> anyhow::Result<()> {
        let _ = ctx;
        Ok(())
    }

    /// Execute the release.
    async fn release(&self, ctx: &RunnerContext) -> anyhow::Result<()>;
}

/// Context provided to a `RunnerDestination` during execution.
pub struct RunnerContext {
    /// The release token for authentication with the server.
    pub release_token: String,
    /// Destination configuration including metadata.
    pub destination: DestinationConfig,
    /// Backend for data access, logging, and temp directory creation.
    pub backend: Box<dyn DestinationBackend>,
}
