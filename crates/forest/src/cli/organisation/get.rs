use anyhow::Context;
use forest_grpc_interface::get_organisation_request;

use crate::{grpc::GrpcClientState, state::State, user_state::UserStateLoaderState};

#[derive(clap::Parser)]
pub struct GetCommand {
    /// Get by organisation ID
    #[arg(long, conflicts_with = "name")]
    id: Option<String>,

    /// Get by organisation name (exact match)
    #[arg(long, conflicts_with = "id")]
    name: Option<String>,
}

impl GetCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
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

        println!("ID:   {}", org.organisation_id);
        println!("Name: {}", org.name);
        if let Some(ts) = org.created_at {
            if let Some(dt) = chrono::DateTime::from_timestamp(ts.seconds, ts.nanos as u32) {
                println!("Created: {}", dt.to_rfc3339());
            }
        }

        Ok(())
    }
}
