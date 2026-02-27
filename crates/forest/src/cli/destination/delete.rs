use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct DeleteCommand {
    #[arg(long)]
    name: String,
}

impl DeleteCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        state
            .grpc_client()
            .delete_destination(&self.name)
            .await
            .context("delete destination")?;

        Ok(())
    }
}
