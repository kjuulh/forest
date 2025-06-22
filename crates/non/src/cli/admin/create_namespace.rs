use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser, Debug)]
pub struct CreateNamespaceCommand {
    namespace: String,
}

impl CreateNamespaceCommand {
    #[tracing::instrument(skip(state), level = "trace")]
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        tracing::info!("creating namespace");

        state
            .grpc_client()
            .create_namespace(&self.namespace)
            .await
            .context("failed to create namespace")?;

        tracing::info!("created namespace");

        Ok(())
    }
}
