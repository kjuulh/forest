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
pub struct UpdateRoleCommand {
    /// Organisation ID or name
    #[arg(long)]
    org: Option<String>,

    /// User ID or username
    #[arg(long)]
    user: Option<String>,

    /// New role: "admin" or "member"
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

impl UpdateRoleCommand {
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
        let role = match &self.role {
            Some(r) => r.clone(),
            None => {
                inquire::Select::new("New role:", vec!["member", "admin"])
                    .prompt()?
                    .to_string()
            }
        };

        let resp = state
            .grpc_client()
            .update_organisation_member_role(&org_id, &user_id, &role)
            .await
            .context("failed to update member role")?;

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
