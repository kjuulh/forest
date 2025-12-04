use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser, Debug)]
pub struct GetComponentCommand {
    #[arg(long)]
    name: String,

    #[arg(long)]
    namespace: String,
}

impl GetComponentCommand {
    #[tracing::instrument(skip(state), level = "trace")]
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        tracing::info!("getting component");

        let Some(component) = state
            .grpc_client()
            .get_component(&self.name, &self.namespace)
            .await
            .context("failed to create namespace")?
        else {
            anyhow::bail!("failed to find component");
        };

        state
            .grpc_client()
            .list_files(&component.id, |file| {
                tracing::info!("file: {}", file.file_path)
            })
            .await?;

        Ok(())
    }
}
