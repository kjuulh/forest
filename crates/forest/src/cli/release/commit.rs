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
    pub(crate) artifact_id: Option<String>,

    #[arg()]
    pub(crate) slug: Option<String>,

    #[arg(long, short = 'o')]
    pub(crate) organisation: Option<String>,

    #[arg(long, short = 'p')]
    pub(crate) project: Option<String>,

    #[arg(long = "ref", short = 'r')]
    pub(crate) r#ref: Option<String>,

    #[arg(long, short = 'e', alias = "env")]
    pub(crate) environment: Option<String>,

    #[arg(long, short = 'd')]
    pub(crate) destination: Option<Vec<String>>,

    /// Skip waiting for the release to complete
    #[arg(long)]
    pub(crate) no_wait: bool,

    /// Force release: cancel queued releases and jump to front of queue
    #[arg(long)]
    pub(crate) force: bool,

    /// Use the project's release pipeline instead of deploying directly
    #[arg(long)]
    pub(crate) pipeline: bool,
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

                // Fetch annotations and release intent states in parallel
                let (release_annotations, intent_states) = tokio::try_join!(
                    grpc.get_release_annotations_by_project(&organisation, &project),
                    grpc.get_release_intent_states(&organisation, Some(&project), true),
                )
                .context("get releases and states")?;

                if release_annotations.is_empty() {
                    anyhow::bail!(
                        "no release annotations found for {}/{}",
                        organisation,
                        project
                    );
                }

                // Determine "current" per destination: the most recently created
                // intent that succeeded on that destination wins.
                // Intents are returned ordered by created DESC, so first seen wins.
                let mut current_per_dest: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                for intent in &intent_states.release_intents {
                    for step in &intent.steps {
                        if step.status == "SUCCEEDED" {
                            current_per_dest
                                .entry(step.destination_name.clone())
                                .or_insert_with(|| intent.artifact_id.clone());
                        }
                    }
                }

                // Build a map: artifact_id -> list of dest display entries
                let mut dest_by_artifact: std::collections::HashMap<
                    String,
                    Vec<DestDisplay>,
                > = std::collections::HashMap::new();
                for intent in &intent_states.release_intents {
                    for step in &intent.steps {
                        let is_current = current_per_dest
                            .get(&step.destination_name)
                            .is_some_and(|aid| *aid == intent.artifact_id);
                        dest_by_artifact
                            .entry(intent.artifact_id.clone())
                            .or_default()
                            .push(DestDisplay {
                                name: step.destination_name.clone(),
                                environment: step.environment.clone(),
                                status: step.status.clone(),
                                is_current,
                            });
                    }
                }

                let display_items: Vec<ReleaseAnnotationDisplay> = release_annotations
                    .into_iter()
                    .map(|ann| {
                        let dests = dest_by_artifact
                            .remove(&ann.artifact_id.to_string())
                            .unwrap_or_default();
                        ReleaseAnnotationDisplay {
                            annotation: ann,
                            dests,
                        }
                    })
                    .collect();

                let choice = inquire::Select::new("Select a release:", display_items).prompt()?;

                choice.annotation.artifact_id
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
                self.force,
                self.pipeline,
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

            // Stream health updates for a short period after release
            self.watch_health(
                &grpc,
                release_result.release_intent_id,
            )
            .await;
        } else {
            tracing::info!("release staged for {artifact_id}");
        }

        Ok(())
    }
}

impl CommitCommand {
    /// Watch health updates for a release intent after deployment completes.
    /// Streams health events for up to 60 seconds or until all destinations are healthy.
    async fn watch_health(
        &self,
        grpc: &crate::grpc::GrpcClient,
        release_intent_id: uuid::Uuid,
    ) {
        let mut client = match grpc.health_client().await {
            Ok(c) => c,
            Err(_) => return, // Health service not available, skip silently
        };

        let timeout = tokio::time::sleep(std::time::Duration::from_secs(120));
        tokio::pin!(timeout);

        eprintln!("\nWatching health...");

        let mut last_status = String::new();
        let poll_interval = std::time::Duration::from_secs(5);

        loop {
            tokio::select! {
                _ = tokio::time::sleep(poll_interval) => {
                    let resp = match client
                        .get_release_health(forest_grpc_interface::GetReleaseHealthRequest {
                            release_intent_id: release_intent_id.to_string(),
                        })
                        .await
                    {
                        Ok(r) => r.into_inner(),
                        Err(_) => continue,
                    };

                    for dest in &resp.destinations {
                        let status_str = match dest.status {
                            x if x == forest_grpc_interface::HealthStatus::Healthy as i32 => "HEALTHY",
                            x if x == forest_grpc_interface::HealthStatus::Progressing as i32 => "PROGRESSING",
                            x if x == forest_grpc_interface::HealthStatus::Degraded as i32 => "DEGRADED",
                            x if x == forest_grpc_interface::HealthStatus::Unhealthy as i32 => "UNHEALTHY",
                            x if x == forest_grpc_interface::HealthStatus::Missing as i32 => "MISSING",
                            _ => "PENDING",
                        };

                        // Only print when status changes
                        let key = format!("{}:{}", dest.destination, status_str);
                        if key != last_status {
                            last_status = key;
                            eprintln!(
                                "  [{env}] {dest}  HEALTH: {status_str}",
                                env = dest.environment,
                                dest = dest.destination,
                            );
                        }
                    }

                    if resp.aggregate_status == forest_grpc_interface::HealthStatus::Healthy as i32 {
                        eprintln!("\nRelease healthy.");
                        return;
                    }
                }
                _ = &mut timeout => {
                    eprintln!("\nHealth watch timed out (120s).");
                    return;
                }
            }
        }
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

struct DestDisplay {
    name: String,
    environment: String,
    status: String,
    is_current: bool,
}

/// Wrapper for ReleaseAnnotation that provides custom Display for inquire Select
struct ReleaseAnnotationDisplay {
    annotation: ReleaseAnnotation,
    dests: Vec<DestDisplay>,
}

impl Display for ReleaseAnnotationDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let created = self.annotation.created_at.format("%Y-%m-%d %H:%M");
        write!(f, "{}: {}", created, self.annotation.context.title)?;

        if !self.dests.is_empty() {
            // Group destinations by environment
            let mut by_env: std::collections::BTreeMap<&str, Vec<&DestDisplay>> =
                std::collections::BTreeMap::new();
            for dest in &self.dests {
                by_env.entry(&dest.environment).or_default().push(dest);
            }

            for (env, dests) in &by_env {
                write!(f, "\n    {}:", env)?;
                for dest in dests {
                    let icon = if dest.is_current {
                        match dest.status.as_str() {
                            "SUCCEEDED" => "✓",
                            "RUNNING" => "▶",
                            "ASSIGNED" => "◉",
                            "QUEUED" => "◌",
                            "FAILED" | "TIMED_OUT" | "CANCELLED" => "✗",
                            _ => "•",
                        }
                    } else {
                        // Previously deployed, now superseded
                        "·"
                    };
                    write!(f, "\n    {icon} {}", dest.name)?;
                }
            }
        }

        Ok(())
    }
}
