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

use anyhow::Context;

use crate::{grpc::GrpcClientState, models::artifacts::ArtifactID, state::State};

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
    environment: Vec<String>,

    #[arg(long, short = 'd')]
    destination: Option<Vec<String>>,
}

impl ReleaseCommand {
    pub async fn execute(&self, state: &State) -> Result<(), anyhow::Error> {
        if self.environment.is_empty() {
            anyhow::bail!("at least one environment is required");
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

                let choice = inquire::Select::new(
                    "select a release",
                    release_annotations
                        .iter()
                        .map(|r| r.slug.to_string())
                        .collect(),
                )
                .prompt()?;

                let release_annotation = release_annotations
                    .iter()
                    .find(|r| r.slug == choice)
                    .expect("slug to match");

                release_annotation.artifact_id
            }
            (None, None, _, _) => {
                todo!(); // TODO: select based on how much namespace / project and ref we receive
            }
        };

        tracing::info!(artifact =% artifact_id, "releasing");

        state
            .grpc_client()
            .release(artifact_id, &destination, &self.environment)
            .await
            .context("release")?;

        tracing::info!("you've released {artifact_id} successfully");

        Ok(())
    }
}
