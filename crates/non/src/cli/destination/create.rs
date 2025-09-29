use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct CreateCommand {
    #[arg(long)]
    name: String,
}

impl CreateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        state
            .grpc_client()
            .create_destination(&self.name)
            .await
            .context("create destination")?;

        Ok(())
    }
}
