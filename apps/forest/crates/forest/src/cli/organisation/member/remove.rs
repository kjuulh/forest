use anyhow::Context;

use crate::{
    cli::output::OutputFormat,
    grpc::GrpcClientState,
    state::State,
    user_state::UserStateLoaderState,
};

#[derive(clap::Parser)]
pub struct RemoveCommand {
    /// Organisation ID or name
    #[arg(long)]
    org: Option<String>,

    /// User ID or username to remove
    #[arg(long)]
    user: Option<String>,
}

impl RemoveCommand {
    pub async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
        let _user_state = state
            .user_state()
            .get_state()
            .await?
            .context("you must be logged in")?;

        let org_id = match &self.org {
            Some(o) => super::resolve_org_id(state, o).await?,
            None => super::prompt_org_select(state, "admin").await?,
        };
        let user_id = match &self.user {
            Some(u) => super::resolve_user_id(state, u).await?,
            None => super::prompt_member_select(state, &org_id).await?,
        };

        state
            .grpc_client()
            .remove_organisation_member(&org_id, &user_id)
            .await
            .context("failed to remove member")?;

        if !matches!(format, OutputFormat::Json) {
            println!("Member removed");
        }

        Ok(())
    }
}
