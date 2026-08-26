use anyhow::Context;

use crate::{
    cli::release::show::{ResolvedRelease, resolve_target},
    grpc::GrpcClientState,
    state::State,
};

/// Approve a plan stage that is parked awaiting approval.
///
/// A pipeline's plan stage with `auto_approve: false` runs its dry-run (for a
/// `forest/terraform@1` destination, `terraform plan`), captures the output, and
/// then sits in `AWAITING_APPROVAL` until a human says yes. The RPC to say yes
/// has existed since plan stages landed, but nothing in the CLI called it — so
/// the guardrail could be armed from a terminal and never released from one.
/// That is what this command is (DATA-655).
///
///   forest release approve                          # the one stage waiting
///   forest release approve <slug|intent-uuid>
///   forest release approve <intent-uuid> --stage plan-prod
///
/// Read the plan before approving it:
///
///   forest release show <intent-uuid>
#[derive(clap::Parser)]
pub struct ApproveCommand {
    /// Release slug or release-intent UUID. Omit for an interactive picker.
    #[arg()]
    target: Option<String>,

    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Stage to approve. Only needed when more than one stage of the release is
    /// awaiting approval — with a single candidate it is inferred.
    #[arg(long)]
    stage: Option<String>,
}

impl ApproveCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let ResolvedRelease {
            intent_id,
            intent_state,
            project,
            ..
        } = resolve(state, self.target.as_deref(), self.organisation.as_deref(), self.project.as_deref())
            .await?;

        let stage_id = super::pick_awaiting_stage(&intent_state, self.stage.as_deref())?;

        state
            .grpc_client()
            .approve_plan_stage(intent_id, &stage_id)
            .await
            .context("approve plan stage")?;

        eprintln!("approved stage '{stage_id}' of {}/{}", project.organisation, project.project);
        eprintln!("  intent: {intent_id}");
        eprintln!("\nthe coordinator activates dependent stages on its next sweep; follow with:");
        eprintln!("  forest release show {intent_id} --follow");

        Ok(())
    }
}

pub(crate) async fn resolve(
    state: &State,
    target: Option<&str>,
    organisation: Option<&str>,
    project: Option<&str>,
) -> anyhow::Result<ResolvedRelease> {
    match target {
        Some(target) => resolve_target(state, target, organisation).await,
        None => {
            crate::cli::release::show::pick_release_interactive(state, organisation, project).await
        }
    }
}
