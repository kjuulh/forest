use anyhow::Context;
use forest_grpc_interface::get_organisation_request;
use serde::Serialize;
use tabled::Tabled;

use crate::{
    cli::output::{self, OutputFormat},
    grpc::GrpcClientState,
    state::State,
    user_state::UserStateLoaderState,
};

#[derive(clap::Parser)]
pub struct GetCommand {
    /// Get by organisation ID
    #[arg(long, conflicts_with = "name")]
    id: Option<String>,

    /// Get by organisation name (exact match)
    #[arg(long, conflicts_with = "id")]
    name: Option<String>,
}

#[derive(Tabled, Serialize)]
struct OrgRow {
    #[tabled(rename = "ID")]
    organisation_id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Created")]
    created_at: String,
}

impl GetCommand {
    pub async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
        let _user_state = state
            .user_state()
            .get_state()
            .await?
            .context("you must be logged in")?;

        let identifier = match (&self.id, &self.name) {
            (Some(id), _) => get_organisation_request::Identifier::OrganisationId(id.clone()),
            (_, Some(name)) => get_organisation_request::Identifier::Name(name.clone()),
            (None, None) => anyhow::bail!("either --id or --name is required"),
        };

        let org = state
            .grpc_client()
            .get_organisation(identifier)
            .await
            .context("failed to get organisation")?
            .ok_or_else(|| anyhow::anyhow!("organisation not found"))?;

        let created_at = org
            .created_at
            .and_then(|ts| chrono::DateTime::from_timestamp(ts.seconds, ts.nanos as u32))
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();

        let rows = vec![OrgRow {
            organisation_id: org.organisation_id,
            name: org.name,
            created_at,
        }];

        print!("{}", output::render(format, &rows));

        Ok(())
    }
}
