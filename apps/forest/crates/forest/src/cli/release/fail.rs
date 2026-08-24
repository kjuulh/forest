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
/// This records the release that did not work — the same intent against the
/// same destination the successful path would have created, born FAILED. Keeping
/// failures out of the release history would leave a project whose successes are
/// releases and whose failures are not, which under-reports it everywhere.
#[derive(clap::Parser)]
pub struct FailCommand {
    /// Release slug to fail, as printed by `forest release annotate`.
    #[arg()]
    slug: String,

    /// Why it failed. Shown to whoever reads the notification, so be concrete:
    /// "ECS rollout did not converge" beats "failed".
    #[arg(long, short = 'r')]
    reason: String,

    /// Destination the deploy was headed for. This or `--environment` is
    /// required: the failed release is recorded against a destination, so there
    /// has to be one to record it against.
    #[arg(long, short = 'd')]
    destination: Option<String>,

    /// Environment the deploy was headed for. Alternative to `--destination`,
    /// resolved the way `forest release --env` resolves it.
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
