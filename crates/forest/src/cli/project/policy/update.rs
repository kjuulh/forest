use anyhow::Context;
use forest_grpc_interface::{update_policy_request, BranchRestrictionConfig, SoakTimeConfig};

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct UpdateCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Policy name to update
    #[arg(long)]
    name: Option<String>,

    /// Enable or disable the policy
    #[arg(long)]
    enabled: Option<bool>,

    /// Policy type (required to update config): soak_time or branch_restriction
    #[arg(long = "type", short = 't')]
    policy_type: Option<String>,

    // ── soak_time fields ────────────────────
    #[arg(long)]
    source_environment: Option<String>,

    #[arg(long)]
    target_environment: Option<String>,

    #[arg(long)]
    duration: Option<i64>,

    // ── branch_restriction fields ───────────
    #[arg(long)]
    branch_pattern: Option<String>,
}

impl UpdateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let organisation = match &self.organisation {
            Some(o) => o.clone(),
            None => prompts::select_organisation(state).await?,
        };

        let project = match &self.project {
            Some(p) => p.clone(),
            None => prompts::select_project(state, &organisation).await?,
        };

        let name = match &self.name {
            Some(n) => n.clone(),
            None => inquire::Text::new("Policy name:").prompt()?,
        };

        let config = match self.policy_type.as_deref() {
            Some("soak_time") => {
                let source = self
                    .source_environment
                    .as_ref()
                    .context("--source-environment is required for soak_time")?;
                let target = self
                    .target_environment
                    .as_ref()
                    .context("--target-environment is required for soak_time")?;
                let duration = self
                    .duration
                    .context("--duration is required for soak_time")?;

                Some(update_policy_request::Config::SoakTime(SoakTimeConfig {
                    source_environment: source.clone(),
                    target_environment: target.clone(),
                    duration_seconds: duration,
                }))
            }
            Some("branch_restriction") => {
                let target = self
                    .target_environment
                    .as_ref()
                    .context("--target-environment is required for branch_restriction")?;
                let pattern = self
                    .branch_pattern
                    .as_ref()
                    .context("--branch-pattern is required for branch_restriction")?;

                Some(update_policy_request::Config::BranchRestriction(
                    BranchRestrictionConfig {
                        target_environment: target.clone(),
                        branch_pattern: pattern.clone(),
                    },
                ))
            }
            Some(other) => {
                anyhow::bail!("unknown policy type: {other}")
            }
            None => None,
        };

        let policy = state
            .grpc_client()
            .update_policy(&organisation, &project, &name, self.enabled, config)
            .await
            .context("update policy")?;

        let status = if policy.enabled {
            "enabled"
        } else {
            "disabled"
        };
        println!("Updated policy '{}' ({})", policy.name, status);

        Ok(())
    }
}
