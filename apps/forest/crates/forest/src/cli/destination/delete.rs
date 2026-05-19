use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct DeleteCommand {
    #[arg(long, short = 'o')]
    organisation: String,

    #[arg(long)]
    name: String,
}

impl DeleteCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        state
            .grpc_client()
            .delete_destination(&self.organisation, &self.name)
            .await
            .context("delete destination")?;

        Ok(())
    }
}
