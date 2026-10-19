pub mod fluxv1;

use forest_grpc_interface::DestinationCapability;

use crate::backend::{DestinationBackend, DestinationConfig};

#[async_trait::async_trait]
pub trait RunnerDestination: Send + Sync {
    fn capabilities(&self) -> Vec<DestinationCapability>;

    async fn prepare(&self, ctx: &RunnerContext) -> anyhow::Result<()> {
        let _ = ctx;
        Ok(())
    }

    async fn release(&self, ctx: &RunnerContext) -> anyhow::Result<()>;

    async fn plan(&self, ctx: &RunnerContext) -> anyhow::Result<Option<String>> {
        let _ = ctx;
        Ok(None)
    }
}

pub struct RunnerContext {
    pub release_token: String,
    pub destination: DestinationConfig,
    pub backend: Box<dyn DestinationBackend>,
}
