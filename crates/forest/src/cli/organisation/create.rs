use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State, user_state::UserStateLoaderState};

#[derive(clap::Parser)]
pub struct CreateCommand {
    /// Name of the organisation to create
    #[arg(long)]
    name: String,
}

impl CreateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let _user_state = state
            .user_state()
            .get_state()
            .await?
            .context("you must be logged in to create an organisation")?;

        let resp = state
            .grpc_client()
            .create_organisation(&self.name)
            .await
            .context("failed to create organisation")?;

        println!(
            "Created organisation '{}' with id {}",
            self.name, resp.organisation_id
        );

        Ok(())
    }
}
