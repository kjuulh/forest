use anyhow::Context;
use serde::Serialize;
use tabled::Tabled;

use crate::{
    cli::output::{self, OutputFormat},
    grpc::GrpcClientState,
    state::State,
    user_state::UserStateLoaderState,
};

#[derive(clap::Parser)]
pub struct CreateCommand {
    /// Name of the organisation to create
    #[arg(long)]
    name: String,
}

#[derive(Tabled, Serialize)]
struct CreatedOrg {
    #[tabled(rename = "ID")]
    organisation_id: String,
    #[tabled(rename = "Name")]
    name: String,
}

impl CreateCommand {
    pub async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
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

        let rows = vec![CreatedOrg {
            organisation_id: resp.organisation_id,
            name: self.name.clone(),
        }];

        print!("{}", output::render(format, &rows));

        Ok(())
    }
}
