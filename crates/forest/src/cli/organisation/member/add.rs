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
pub struct AddCommand {
    /// Organisation ID or name
    #[arg(long)]
    org: Option<String>,

    /// User ID or username to add
    #[arg(long)]
    user: Option<String>,

    /// Role: "admin" or "member"
    #[arg(long)]
    role: Option<String>,
}

#[derive(Tabled, Serialize)]
struct MemberRow {
    #[tabled(rename = "User ID")]
    user_id: String,
    #[tabled(rename = "Username")]
    username: String,
    #[tabled(rename = "Role")]
    role: String,
}

impl AddCommand {
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
            None => super::prompt_user_search(state).await?,
        };
        let role = match &self.role {
            Some(r) => r.clone(),
            None => {
                inquire::Select::new("Role:", vec!["member", "admin"])
                    .prompt()?
                    .to_string()
            }
        };

        let resp = state
            .grpc_client()
            .add_organisation_member(&org_id, &user_id, &role)
            .await
            .context("failed to add member")?;

        let member = resp.member.context("no member in response")?;

        let rows = vec![MemberRow {
            user_id: member.user_id,
            username: member.username,
            role: member.role,
        }];

        print!("{}", output::render(format, &rows));

        Ok(())
    }
}
