use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

/// Report that the deploy an annotation announced never happened.
///
/// The counterpart to annotating early. A project whose rollout forest does not
/// perform announces a deploy up front — subscribers see it as pending — and
/// releases once its own CI has finished. When that CI fails there is no release
/// intent for a destination to fail, so nothing contradicts the pending state
/// and it sits looking in-flight forever (DATA-637).
///
///   forest release fail <slug> --reason "ECS rollout did not converge"
///
/// This records no release. Nothing was released, and a failed release in the
/// history for a release that never ran is a lie every dashboard would repeat.
#[derive(clap::Parser)]
pub struct FailCommand {
    /// Release slug to fail, as printed by `forest release annotate`.
    #[arg()]
    slug: String,

    /// Why it failed. Shown to whoever reads the notification, so be concrete:
    /// "ECS rollout did not converge" beats "failed".
    #[arg(long, short = 'r')]
    reason: String,

    /// Destination the deploy was headed for. Cosmetic — it lets the
    /// notification name the target the way a real release failure would.
    #[arg(long, short = 'd')]
    destination: Option<String>,

    /// Environment the deploy was headed for. Cosmetic, as with `--destination`.
    #[arg(long, short = 'e', alias = "env")]
    environment: Option<String>,
}

impl FailCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        state
            .grpc_client()
            .report_release_failed(
                &self.slug,
                &self.reason,
                self.destination.clone(),
                self.environment.clone(),
            )
            .await
            .context("report release failed")?;

        eprintln!("marked release {} as failed: {}", self.slug, self.reason);

        Ok(())
    }
}
