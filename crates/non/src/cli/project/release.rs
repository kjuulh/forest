// # ARCHITECTURE:
//
// destinations are viewable in two ways. either destination first, or project first. I.e. For which destinations what is our current state, and for which projects what is currently released.
//
// ## Destinations
//
// 1. Destination / Project (symlink) / Refs (commit (HEAD, history))
// 2. Namespace / Project / Refs (commits, branches) / Destinations
//
// Destinations will receive hooks for each change to the refs

use std::fmt::Display;

use anyhow::Context;

use crate::{
    grpc::GrpcClientState,
    models::{artifacts::ArtifactID, release_annotation::ReleaseAnnotation},
    state::State,
};

#[derive(clap::Parser)]
pub struct ReleaseCommand {
    #[arg(long = "artifact-id", alias = "id")]
    artifact_id: Option<String>,

    #[arg()]
    slug: Option<String>,

    #[arg(long, short = 'n')]
    namespace: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    #[arg(long = "ref", short = 'r')]
    r#ref: Option<String>,

    #[arg(long, short = 'e', alias = "env")]
    environment: String,

    #[arg(long, short = 'd')]
    destination: Option<Vec<String>>,

    /// Wait for the release to complete (SUCCESS or FAILURE)
    #[arg(long, short = 'w')]
    wait: bool,
}

impl ReleaseCommand {
    pub async fn execute(&self, state: &State) -> Result<(), anyhow::Error> {
        if self.environment.is_empty() {
            anyhow::bail!("environment is required");
        }

        let destination = self.destination.clone().unwrap_or_default();

        let artifact_id: ArtifactID = match (
            &self.artifact_id,
            &self.slug,
            &self.namespace,
            &self.project,
        ) {
            (Some(artifact_id), _, _, _) => artifact_id.parse().context("artifact id")?,
            (_, Some(slug), _, _) => {
                state
                    .grpc_client()
                    .get_release_annotation_by_slug(slug)
                    .await
                    .context("get release annotation by slug")?
                    .artifact_id
            }
            (_, _, Some(namespace), Some(project)) => {
                let release_annotations = state
                    .grpc_client()
                    .get_release_annotations_by_project(namespace, project)
                    .await
                    .context("get releases by namespace and project")?;

                let display_items: Vec<ReleaseAnnotationDisplay> = release_annotations
                    .into_iter()
                    .map(ReleaseAnnotationDisplay)
                    .collect();

                let choice = inquire::Select::new("select a release", display_items).prompt()?;

                choice.0.artifact_id
            }
            (None, None, _, _) => {
                todo!(); // TODO: select based on how much namespace / project and ref we receive
            }
        };

        tracing::info!(artifact =% artifact_id, environment =% self.environment, "releasing");

        let release_result = state
            .grpc_client()
            .release(
                artifact_id,
                &destination,
                std::slice::from_ref(&self.environment),
            )
            .await
            .context("release")?;

        tracing::info!(
            release_intent_id =% release_result.release_intent_id,
            destinations = ?release_result.releases.iter().map(|r| &r.destination).collect::<Vec<_>>(),
            "release intent created"
        );

        if self.wait {
            println!("Waiting for release to complete (streaming logs)...\n");

            let result = state
                .grpc_client()
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

/// Wrapper for ReleaseAnnotation that provides custom Display for inquire Select
struct ReleaseAnnotationDisplay(ReleaseAnnotation);

impl Display for ReleaseAnnotationDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let annotation = &self.0;

        // Format: slug (created_at) [dest1@env1, dest2@env2, ...]
        let created = annotation.created_at.format("%Y-%m-%d %H:%M");
        write!(f, "{}: {}", created, annotation.context.title)?;

        if !annotation.destinations.is_empty() {
            write!(f, "\n    destinations: ")?;

            for (i, dest) in annotation.destinations.iter().enumerate() {
                if i > 0 {
                    write!(f, "\n    - ")?;
                }
                write!(f, "{}@{}", dest.name, dest.environment)?;
            }
            writeln!(f)?;
        }

        Ok(())
    }
}
