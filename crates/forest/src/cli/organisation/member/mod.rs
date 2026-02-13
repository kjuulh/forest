mod add;
mod list;
mod remove;
mod update_role;

use std::fmt;

use anyhow::Context;
use forest_grpc_interface::{get_organisation_request, get_user_request};

use crate::{cli::output::OutputFormat, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct MemberCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Add a member to an organisation
    Add(add::AddCommand),
    /// Remove a member from an organisation
    Remove(remove::RemoveCommand),
    /// Update a member's role
    UpdateRole(update_role::UpdateRoleCommand),
    /// List members of an organisation
    List(list::ListCommand),
}

impl MemberCommand {
    pub async fn execute(&self, state: &State, format: &OutputFormat) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Add(cmd) => cmd.execute(state, format).await,
            Commands::Remove(cmd) => cmd.execute(state, format).await,
            Commands::UpdateRole(cmd) => cmd.execute(state, format).await,
            Commands::List(cmd) => cmd.execute(state, format).await,
        }
    }
}

pub(crate) async fn resolve_org_id(state: &State, org: &str) -> anyhow::Result<String> {
    if uuid::Uuid::try_parse(org).is_ok() {
        return Ok(org.to_string());
    }
    let org_obj = state
        .grpc_client()
        .get_organisation(get_organisation_request::Identifier::Name(org.to_string()))
        .await
        .context("failed to look up organisation by name")?
        .ok_or_else(|| anyhow::anyhow!("organisation '{}' not found", org))?;
    Ok(org_obj.organisation_id)
}

struct OrgChoice {
    organisation_id: String,
    name: String,
}

impl fmt::Display for OrgChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// Fetches the user's organisations (optionally filtered by role) and presents
/// an interactive select prompt. Returns the selected organisation ID.
pub(crate) async fn prompt_org_select(
    state: &State,
    role_filter: &str,
) -> anyhow::Result<String> {
    let resp = state
        .grpc_client()
        .list_my_organisations(role_filter)
        .await
        .context("failed to list your organisations")?;

    if resp.organisations.is_empty() {
        if role_filter.is_empty() {
            anyhow::bail!("you are not a member of any organisations");
        } else {
            anyhow::bail!("you are not an {role_filter} of any organisations");
        }
    }

    let choices: Vec<OrgChoice> = resp
        .organisations
        .into_iter()
        .map(|o| OrgChoice {
            organisation_id: o.organisation_id,
            name: o.name,
        })
        .collect();

    let selected = inquire::Select::new("Organisation:", choices).prompt()?;
    Ok(selected.organisation_id)
}

struct MemberChoice {
    user_id: String,
    username: String,
    role: String,
}

impl fmt::Display for MemberChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.username, self.role)
    }
}

/// Fetches the members of an organisation and presents an interactive select
/// prompt. Returns the selected user ID.
pub(crate) async fn prompt_member_select(
    state: &State,
    organisation_id: &str,
) -> anyhow::Result<String> {
    let resp = state
        .grpc_client()
        .list_organisation_members(organisation_id, 200, "")
        .await
        .context("failed to list organisation members")?;

    if resp.members.is_empty() {
        anyhow::bail!("this organisation has no members");
    }

    let choices: Vec<MemberChoice> = resp
        .members
        .into_iter()
        .map(|m| MemberChoice {
            user_id: m.user_id,
            username: m.username,
            role: m.role,
        })
        .collect();

    let selected = inquire::Select::new("Member:", choices).prompt()?;
    Ok(selected.user_id)
}

pub(crate) async fn resolve_user_id(state: &State, user: &str) -> anyhow::Result<String> {
    if uuid::Uuid::try_parse(user).is_ok() {
        return Ok(user.to_string());
    }
    let user_obj = state
        .grpc_client()
        .get_user(get_user_request::Identifier::Username(user.to_string()))
        .await
        .context("failed to look up user by username")?
        .ok_or_else(|| anyhow::anyhow!("user '{}' not found", user))?;
    Ok(user_obj.user_id)
}
