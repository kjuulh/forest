use std::fmt::Display;

use anyhow::Context;

use crate::{
    grpc::{GetProjectsQuery, GrpcClientState},
    models::{artifacts::ArtifactID, release_annotation::ReleaseAnnotation},
    state::State,
};

#[derive(Clone, Default, clap::Args)]
pub struct CommitCommand {
    #[arg(long = "artifact-id", alias = "id")]
    artifact_id: Option<String>,

    #[arg()]
    slug: Option<String>,

    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    #[arg(long = "ref", short = 'r')]
    r#ref: Option<String>,

    #[arg(long, short = 'e', alias = "env")]
    environment: Option<String>,

    #[arg(long, short = 'd')]
    destination: Option<Vec<String>>,

    /// Skip waiting for the release to complete
    #[arg(long)]
    no_wait: bool,
}

impl CommitCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let grpc = state.grpc_client();
        let destination = self.destination.clone().unwrap_or_default();

        let artifact_id: ArtifactID = match (&self.artifact_id, &self.slug) {
            (Some(artifact_id), _) => artifact_id.parse().context("artifact id")?,
            (_, Some(slug)) => {
                grpc.get_release_annotation_by_slug(slug)
                    .await
                    .context("get release annotation by slug")?
                    .artifact_id
            }
            (None, None) => {
                // Resolve organisation: from flag or interactive prompt
                let organisation = match &self.organisation {
                    Some(org) => org.clone(),
                    None => prompt_org_select(state).await?,
                };

                // Resolve project: from flag or interactive prompt
                let project = match &self.project {
                    Some(proj) => proj.clone(),
                    None => prompt_project_select(state, &organisation).await?,
                };

                // Select a release annotation
                let release_annotations = grpc
                    .get_release_annotations_by_project(&organisation, &project)
                    .await
                    .context("get releases by organisation and project")?;

                if release_annotations.is_empty() {
                    anyhow::bail!(
                        "no release annotations found for {}/{}",
                        organisation,
                        project
                    );
                }

                let display_items: Vec<ReleaseAnnotationDisplay> = release_annotations
                    .into_iter()
                    .map(ReleaseAnnotationDisplay)
                    .collect();

                let choice = inquire::Select::new("Select a release:", display_items).prompt()?;

                choice.0.artifact_id
            }
        };

        // Resolve environment: from flag or interactive prompt
        let environment = match &self.environment {
            Some(env) if !env.is_empty() => env.clone(),
            _ => {
                // We need the org to list destinations. Try to get it from flags
                // or from the artifact annotation.
                let organisation = if let Some(org) = &self.organisation {
                    org.clone()
                } else {
                    // Fetch the annotation to get the org
                    let orgs = grpc
                        .get_organisations()
                        .await
                        .context("get organisations")?;
                    if orgs.len() == 1 {
                        orgs.into_iter().next().unwrap().to_string()
                    } else {
                        prompt_org_select(state).await?
                    }
                };

                prompt_environment_select(state, &organisation).await?
            }
        };

        tracing::info!(artifact =% artifact_id, environment =% environment, "releasing");

        let release_result = grpc
            .release(
                artifact_id,
                &destination,
                std::slice::from_ref(&environment),
            )
            .await
            .context("release")?;

        tracing::info!(
            release_intent_id =% release_result.release_intent_id,
            destinations = ?release_result.releases.iter().map(|r| &r.destination).collect::<Vec<_>>(),
            "release intent created"
        );

        if !self.no_wait {
            println!("Waiting for release to complete (streaming logs)...\n");

            let result = grpc
                .wait_release(release_result.release_intent_id)
                .await
                .context("wait_release")?;

            println!(); // Empty line after logs

            // Report results for each destination
            let mut any_failed = false;
            for dest_result in &result.destinations {
                if dest_result.status.is_success() {
                    println!(
                        "Release completed successfully for destination: {}",
                        dest_result.destination
                    );
                } else {
                    eprintln!(
                        "Release failed for destination: {} with status: {}",
                        dest_result.destination, dest_result.status
                    );
                    any_failed = true;
                }
            }

            if any_failed {
                anyhow::bail!("one or more releases failed");
            }
        } else {
            tracing::info!("release staged for {artifact_id}");
        }

        Ok(())
    }
}

/// Prompt user to select an organisation from their memberships.
async fn prompt_org_select(state: &State) -> anyhow::Result<String> {
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

struct OrgChoice {
    name: String,
}

impl Display for OrgChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// Prompt user to select a project within an organisation.
async fn prompt_project_select(state: &State, organisation: &str) -> anyhow::Result<String> {
    let projects = state
        .grpc_client()
        .get_projects(GetProjectsQuery::Organisation(
            organisation.to_string().into(),
        ))
        .await
        .context("failed to list projects")?;

    if projects.is_empty() {
        anyhow::bail!("no projects found for organisation '{}'", organisation);
    }

    if projects.len() == 1 {
        return Ok(projects.into_iter().next().unwrap().to_string());
    }

    let choices: Vec<String> = projects.iter().map(|p| p.to_string()).collect();
    let selected = inquire::Select::new("Project:", choices).prompt()?;
    Ok(selected)
}

/// Prompt user to select an environment from available destinations.
async fn prompt_environment_select(state: &State, organisation: &str) -> anyhow::Result<String> {
    let destinations = state
        .grpc_client()
        .get_destinations(organisation)
        .await
        .context("failed to list destinations")?;

    if destinations.is_empty() {
        anyhow::bail!("no destinations found for organisation '{}'", organisation);
    }

    // Collect unique environments
    let mut environments: Vec<String> = destinations
        .iter()
        .map(|d| d.environment.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    environments.sort();

    if environments.len() == 1 {
        return Ok(environments.into_iter().next().unwrap());
    }

    let selected = inquire::Select::new("Environment:", environments).prompt()?;
    Ok(selected)
}

/// Wrapper for ReleaseAnnotation that provides custom Display for inquire Select
struct ReleaseAnnotationDisplay(ReleaseAnnotation);

impl Display for ReleaseAnnotationDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let annotation = &self.0;

        let created = annotation.created_at.format("%Y-%m-%d %H:%M");
        write!(f, "{}: {}", created, annotation.context.title)?;

        if !annotation.destinations.is_empty() {
            // Group destinations by environment
            let mut by_env: std::collections::BTreeMap<&str, Vec<&str>> =
                std::collections::BTreeMap::new();
            for dest in &annotation.destinations {
                by_env
                    .entry(&dest.environment)
                    .or_default()
                    .push(&dest.name);
            }

            for (env, names) in &by_env {
                write!(f, "\n    {}:", env)?;
                for name in names {
                    write!(f, "\n    - {}", name)?;
                }
            }
        }

        Ok(())
    }
}
