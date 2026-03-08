use anyhow::Context;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct ListCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,
}

impl ListCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let organisation = match &self.organisation {
            Some(o) => o.clone(),
            None => prompts::select_organisation(state).await?,
        };

        let project = match &self.project {
            Some(p) => p.clone(),
            None => prompts::select_project(state, &organisation).await?,
        };

        let policies = state
            .grpc_client()
            .list_policies(&organisation, &project)
            .await
            .context("list policies")?;

        if policies.is_empty() {
            println!("No policies found");
            return Ok(());
        }

        eprintln!("policies\n");

        for policy in policies {
            let status = if policy.enabled {
                "enabled"
            } else {
                "disabled"
            };
            let type_name = match policy.policy_type {
                1 => "soak_time",
                2 => "branch_restriction",
                _ => "unknown",
            };
            println!("{} ({}, {})", policy.name, type_name, status);

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
                None => {}
            }
            println!("  id:             {}", policy.id);
            println!();
        }

        Ok(())
    }
}
