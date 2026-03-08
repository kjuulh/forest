use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct DeleteCommand {
    #[arg(long)]
    id: Option<String>,
}

impl DeleteCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let id = match &self.id {
            Some(id) => id.clone(),
            None => inquire::Text::new("Environment ID:").prompt()?,
        };

        state
            .grpc_client()
            .delete_environment(&id)
            .await
            .context("delete environment")?;

        println!("Deleted environment {}", id);

        Ok(())
    }
}
