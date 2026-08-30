use crate::{
    cli::release::{
        annotate::AnnotateCommand, approve::ApproveCommand, commit::CommitCommand,
        create::CreateCommand, fail::FailCommand, prepare::PrepareCommand, reject::RejectCommand,
        show::ShowCommand,
    },
    state::State,
};

pub(crate) mod annotate;
pub(crate) mod approve;
pub(crate) mod commit;
mod create;
pub(crate) mod detect;
mod fail;
pub(crate) mod prepare;
pub(crate) mod reject;
pub(crate) mod show;

#[derive(clap::Parser)]
#[clap(subcommand_required = false, args_conflicts_with_subcommands = true)]
pub struct ReleaseCommand {
    #[command(subcommand)]
    commands: Option<Commands>,

    #[command(flatten)]
    release: Option<CommitCommand>,
}

#[allow(clippy::large_enum_variant)]
#[derive(clap::Subcommand)]
pub enum Commands {
    /// Generate deployment manifests by invoking component hooks
    Prepare(PrepareCommand),
    /// Upload deployment artifacts and create a release annotation
    Annotate(AnnotateCommand),
    /// Execute the release (deploy to destinations)
    Release(CommitCommand),
    /// Prepare, annotate, and release in one step (annotation-only, no auto-release from triggers).
    Create(CreateCommand),
    /// Show detail (header, stages, destinations, plan output, deploy logs) for a release.
    Show(ShowCommand),
    /// Report that the deploy an annotation announced never happened.
    Fail(FailCommand),
    /// Approve a plan stage parked awaiting approval, letting the apply proceed.
    Approve(ApproveCommand),
    /// Reject a plan stage, cancelling it and everything downstream.
    Reject(RejectCommand),
}

impl ReleaseCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Some(Commands::Prepare(cmd)) => {
                cmd.execute(state).await?;
                eprintln!(
                    "\nhint: use 'forest release create --env <env>' to prepare, annotate, and release in one step"
                );
            }
            Some(Commands::Annotate(cmd)) => cmd.execute(state).await?,
            Some(Commands::Release(cmd)) => cmd.execute(state).await?,
            Some(Commands::Create(cmd)) => cmd.execute(state).await?,
            Some(Commands::Show(cmd)) => cmd.execute(state).await?,
            Some(Commands::Fail(cmd)) => cmd.execute(state).await?,
            Some(Commands::Approve(cmd)) => cmd.execute(state).await?,
            Some(Commands::Reject(cmd)) => cmd.execute(state).await?,
            None => {
                let cmd = self.release.as_ref().cloned().unwrap_or_default();
                cmd.execute(state).await?
            }
        }

        Ok(())
    }
}

/// Pick the stage an approve/reject should act on.
///
/// With `--stage` given, verify it really is awaiting approval rather than
/// letting the server reject it with a less helpful message. Without it, infer:
/// exactly one candidate is the overwhelmingly common case (a pipeline parks one
/// gate at a time), and guessing between several would be worse than asking.
pub(crate) fn pick_awaiting_stage(
    intent_state: &forest_grpc_interface::ReleaseIntentState,
    requested: Option<&str>,
) -> anyhow::Result<String> {
    // Read `approval_status`, not `status`. A stage parked on its gate stays
    // `ACTIVE` — approval is tracked alongside the stage status, not as a value
    // of it, and the `AWAITING_APPROVAL` variant of PipelineRunStageStatus is
    // never emitted.
    let awaiting: Vec<&str> = intent_state
        .stages
        .iter()
        .filter(|s| s.approval_status.as_deref() == Some("AWAITING_APPROVAL"))
        .map(|s| s.stage_id.as_str())
        .collect();

    if let Some(stage) = requested {
        if !intent_state.stages.iter().any(|s| s.stage_id == stage) {
            anyhow::bail!(
                "release has no stage '{stage}' (stages: {})",
                stage_list(intent_state)
            );
        }
        if !awaiting.contains(&stage) {
            anyhow::bail!(
                "stage '{stage}' is not awaiting approval{}",
                if awaiting.is_empty() {
                    String::new()
                } else {
                    format!(" (awaiting: {})", awaiting.join(", "))
                }
            );
        }
        return Ok(stage.to_string());
    }

    match awaiting.as_slice() {
        [] => Err(anyhow::anyhow!(
            "no stage of this release is awaiting approval (stages: {})",
            stage_list(intent_state)
        )),
        [only] => Ok(only.to_string()),
        many => Err(anyhow::anyhow!(
            "{} stages are awaiting approval; pass --stage <{}>",
            many.len(),
            many.join("|")
        )),
    }
}

fn stage_list(intent_state: &forest_grpc_interface::ReleaseIntentState) -> String {
    if intent_state.stages.is_empty() {
        return "none — this is not a pipeline release".to_string();
    }
    intent_state
        .stages
        .iter()
        .map(|s| s.stage_id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use forest_grpc_interface::{PipelineStageState, ReleaseIntentState};

    fn stage(id: &str, approval: Option<&str>) -> PipelineStageState {
        PipelineStageState {
            stage_id: id.into(),
            depends_on: vec![],
            stage_type: 0,
            status: 0,
            queued_at: None,
            started_at: None,
            completed_at: None,
            error_message: None,
            environment: None,
            duration_seconds: None,
            wait_until: None,
            release_ids: vec![],
            approval_status: approval.map(|a| a.to_string()),
            auto_approve: None,
        }
    }

    fn intent(stages: Vec<PipelineStageState>) -> ReleaseIntentState {
        ReleaseIntentState {
            release_intent_id: "00000000-0000-0000-0000-000000000000".into(),
            artifact_id: String::new(),
            project: String::new(),
            created_at: String::new(),
            stages,
            steps: vec![],
        }
    }

    #[test]
    fn infers_the_single_stage_awaiting_approval() {
        let i = intent(vec![
            stage("plan-dev", Some("APPROVED")),
            stage("deploy-dev", None),
            stage("plan-prod", Some("AWAITING_APPROVAL")),
        ]);
        assert_eq!(pick_awaiting_stage(&i, None).unwrap(), "plan-prod");
    }

    #[test]
    fn refuses_to_guess_between_several() {
        let i = intent(vec![
            stage("plan-a", Some("AWAITING_APPROVAL")),
            stage("plan-b", Some("AWAITING_APPROVAL")),
        ]);
        let err = format!("{:#}", pick_awaiting_stage(&i, None).unwrap_err());
        assert!(err.contains("--stage"), "{err}");
    }

    #[test]
    fn errors_when_nothing_is_waiting() {
        let i = intent(vec![stage("plan-dev", Some("APPROVED"))]);
        let err = format!("{:#}", pick_awaiting_stage(&i, None).unwrap_err());
        assert!(err.contains("no stage"), "{err}");
        assert!(err.contains("plan-dev"), "{err}");
    }

    #[test]
    fn explicit_stage_must_exist() {
        let i = intent(vec![stage("plan-dev", Some("AWAITING_APPROVAL"))]);
        let err = format!("{:#}", pick_awaiting_stage(&i, Some("nope")).unwrap_err());
        assert!(err.contains("no stage 'nope'"), "{err}");
    }

    #[test]
    fn explicit_stage_must_be_awaiting() {
        let i = intent(vec![
            stage("plan-dev", Some("APPROVED")),
            stage("plan-prod", Some("AWAITING_APPROVAL")),
        ]);
        let err = format!(
            "{:#}",
            pick_awaiting_stage(&i, Some("plan-dev")).unwrap_err()
        );
        assert!(err.contains("not awaiting approval"), "{err}");
        assert!(err.contains("plan-prod"), "{err}");
    }

    #[test]
    fn explicit_stage_that_is_awaiting_is_accepted() {
        let i = intent(vec![stage("plan-prod", Some("AWAITING_APPROVAL"))]);
        assert_eq!(
            pick_awaiting_stage(&i, Some("plan-prod")).unwrap(),
            "plan-prod"
        );
    }

    #[test]
    fn non_pipeline_release_says_so() {
        let i = intent(vec![]);
        let err = format!("{:#}", pick_awaiting_stage(&i, None).unwrap_err());
        assert!(err.contains("not a pipeline release"), "{err}");
    }
}
