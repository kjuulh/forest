use anyhow::Context;
use forest_grpc_interface::{create_policy_request, BranchRestrictionConfig, SoakTimeConfig};

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct CreateCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Policy name (unique per project)
    #[arg(long)]
    name: Option<String>,

    /// Policy type: soak_time or branch_restriction
    #[arg(long = "type", short = 't')]
    policy_type: String,

    // ── soak_time fields ────────────────────
    /// Source environment (for soak_time)
    #[arg(long)]
    source_environment: Option<String>,

    /// Target environment
    #[arg(long)]
    target_environment: Option<String>,

    /// Duration in seconds (for soak_time)
    #[arg(long)]
    duration: Option<i64>,

    // ── branch_restriction fields ───────────
    /// Branch pattern regex (for branch_restriction)
    #[arg(long)]
    branch_pattern: Option<String>,
}

impl CreateCommand {
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

        let (policy_type_int, config) = match self.policy_type.as_str() {
            "soak_time" => {
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

                (
                    1,
                    Some(create_policy_request::Config::SoakTime(SoakTimeConfig {
                        source_environment: source.clone(),
                        target_environment: target.clone(),
                        duration_seconds: duration,
                    })),
                )
            }
            "branch_restriction" => {
                let target = self
                    .target_environment
                    .as_ref()
                    .context("--target-environment is required for branch_restriction")?;
                let pattern = self
                    .branch_pattern
                    .as_ref()
                    .context("--branch-pattern is required for branch_restriction")?;

                (
                    2,
                    Some(create_policy_request::Config::BranchRestriction(
                        BranchRestrictionConfig {
                            target_environment: target.clone(),
                            branch_pattern: pattern.clone(),
                        },
                    )),
                )
            }
            other => anyhow::bail!("unknown policy type: {other} (expected: soak_time, branch_restriction)"),
        };

        let policy = state
            .grpc_client()
            .create_policy(&organisation, &project, &name, policy_type_int, config)
            .await
            .context("create policy")?;

        println!("Created policy '{}'", policy.name);
        print_policy_details(&policy);

        Ok(())
    }
}

fn print_policy_details(policy: &forest_grpc_interface::Policy) {
    let type_name = match policy.policy_type {
        1 => "soak_time",
        2 => "branch_restriction",
        _ => "unknown",
    };
    println!("  type:           {type_name}");

    match &policy.config {
        Some(forest_grpc_interface::policy::Config::SoakTime(st)) => {
            println!("  source env:     {}", st.source_environment);
            println!("  target env:     {}", st.target_environment);
            println!("  duration:       {}s", st.duration_seconds);
        }
        Some(forest_grpc_interface::policy::Config::BranchRestriction(br)) => {
            println!("  target env:     {}", br.target_environment);
            println!("  branch pattern: {}", br.branch_pattern);
        }
        Some(forest_grpc_interface::policy::Config::ExternalApproval(ea)) => {
            println!("  target env:          {}", ea.target_environment);
            println!("  required approvals:  {}", ea.required_approvals);
        }
        None => {}
    }
}
