use anyhow::Context;

use crate::{cli::release::show::ResolvedRelease, grpc::GrpcClientState, state::State};

/// Reject a plan stage that is parked awaiting approval.
///
/// The other half of `forest release approve`. Rejecting fails the stage and
/// transitively cancels everything downstream of it, so a plan you do not like
/// never becomes an apply.
///
///   forest release reject <intent-uuid> --reason "destroys the prod volumes"
#[derive(clap::Parser)]
pub struct RejectCommand {
    /// Release slug or release-intent UUID. Omit for an interactive picker.
    #[arg()]
    target: Option<String>,

    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Stage to reject. Only needed when more than one stage of the release is
    /// awaiting approval — with a single candidate it is inferred.
    #[arg(long)]
    stage: Option<String>,

    /// Why it was rejected. Recorded on the stage, so be concrete: whoever
    /// re-runs this needs to know what to change.
    #[arg(long, short = 'r')]
    reason: Option<String>,
}

impl RejectCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let ResolvedRelease {
            intent_id,
            intent_state,
            project,
            ..
        } = super::approve::resolve(
            state,
            self.target.as_deref(),
            self.organisation.as_deref(),
            self.project.as_deref(),
        )
        .await?;

        let stage_id = super::pick_awaiting_stage(&intent_state, self.stage.as_deref())?;

        state
            .grpc_client()
            .reject_plan_stage(intent_id, &stage_id, self.reason.as_deref())
            .await
            .context("reject plan stage")?;

        eprintln!(
            "rejected stage '{stage_id}' of {}/{}",
            project.organisation, project.project
        );
        eprintln!("  intent: {intent_id}");
        if let Some(reason) = &self.reason {
            eprintln!("  reason: {reason}");
        }
        eprintln!("\nstages downstream of it are cancelled.");

        Ok(())
    }
}
