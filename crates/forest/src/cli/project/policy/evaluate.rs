use anyhow::Context;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct EvaluateCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Target environment to evaluate policies for
    #[arg(long)]
    target_environment: String,

    /// Branch to check against branch restriction policies
    #[arg(long)]
    branch: Option<String>,
}

impl EvaluateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let organisation = match &self.organisation {
            Some(o) => o.clone(),
            None => prompts::select_organisation(state).await?,
        };

        let project = match &self.project {
            Some(p) => p.clone(),
            None => prompts::select_project(state, &organisation).await?,
        };

        let resp = state
            .grpc_client()
            .evaluate_policies(
                &organisation,
                &project,
                &self.target_environment,
                self.branch.clone(),
            )
            .await
            .context("evaluate policies")?;

        if resp.evaluations.is_empty() {
            println!("No policies apply to environment '{}'", self.target_environment);
            return Ok(());
        }

        for eval in &resp.evaluations {
            let status = if eval.passed { "PASS" } else { "FAIL" };
            let type_name = match eval.policy_type {
                1 => "soak_time",
                2 => "branch_restriction",
                _ => "unknown",
            };
            println!("[{status}] {} ({type_name})", eval.policy_name);
            println!("       {}", eval.reason);
        }

        println!();
        if resp.all_passed {
            println!("All policies passed for '{}'", self.target_environment);
        } else {
            println!(
                "Some policies FAILED for '{}' — deployment would be blocked",
                self.target_environment
            );
        }

        Ok(())
    }
}
