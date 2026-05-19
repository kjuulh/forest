use std::fmt::Display;

use anyhow::Context;

use crate::{
    grpc::{GetProjectsQuery, GrpcClientState},
    state::State,
};

/// Prompt user to select an organisation from their memberships.
/// If they belong to exactly one, it is returned without prompting.
pub async fn select_organisation(state: &State) -> anyhow::Result<String> {
    let resp = state
        .grpc_client()
        .list_my_organisations("")
        .await
        .context("failed to list your organisations")?;

    if resp.organisations.is_empty() {
        anyhow::bail!("you are not a member of any organisations");
    }

    if resp.organisations.len() == 1 {
        return Ok(resp.organisations.into_iter().next().unwrap().name);
    }

    let choices: Vec<OrgChoice> = resp
        .organisations
        .into_iter()
        .map(|o| OrgChoice { name: o.name })
        .collect();

    let selected = inquire::Select::new("Organisation:", choices).prompt()?;
    Ok(selected.name)
}

/// Prompt user to select a project within an organisation.
/// If there's exactly one, it is returned without prompting.
pub async fn select_project(state: &State, organisation: &str) -> anyhow::Result<String> {
    let projects = state
        .grpc_client()
        .get_projects(GetProjectsQuery::Organisation(
            organisation.to_string().into(),
        ))
        .await
        .context("failed to list projects")?;

    if projects.is_empty() {
        anyhow::bail!(
            "no projects found for organisation '{}'",
            organisation
        );
    }

    if projects.len() == 1 {
        return Ok(projects.into_iter().next().unwrap().to_string());
    }

    let choices: Vec<String> = projects.iter().map(|p| p.to_string()).collect();
    let selected = inquire::Select::new("Project:", choices).prompt()?;
    Ok(selected)
}

struct OrgChoice {
    name: String,
}

impl Display for OrgChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}
