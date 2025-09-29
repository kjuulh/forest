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

    #[arg(long, short = 'd')]
    destination: Vec<String>,
}

impl ReleaseCommand {
    pub async fn execute(&self, state: &State) -> Result<(), anyhow::Error> {
        if self.destination.is_empty() {
            anyhow::bail!("a destination is required for deployment")
        }

        let artifact_id: ArtifactID = match (&self.artifact_id, &self.slug) {
            (Some(artifact_id), _) => artifact_id.parse().context("artifact id")?,
            (_, Some(slug)) => {
                state
                    .grpc_client()
                    .get_release_annotation_by_slug(slug)
                    .await
                    .context("get release annotation by slug")?
                    .artifact_id
            }
            (None, None) => {
                todo!(); // TODO: select based on how much namespace / project and ref we receive
            }
        };

        tracing::info!("found artifact: {}", artifact_id);

        tracing::info!("releasing");

        state
            .grpc_client()
            .release(artifact_id, &self.destination)
            .await
            .context("release")?;

        tracing::info!("you've released {artifact_id} successfully");

        Ok(())
    }
}
