use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct DeleteTokenCommand {
    /// Token ID to delete
    token_id: String,
}

impl DeleteTokenCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        state
            .grpc_client()
            .delete_personal_access_token(&self.token_id)
            .await
            .context("failed to delete token")?;

        eprintln!("Token {} deleted", self.token_id);

        Ok(())
    }
}
