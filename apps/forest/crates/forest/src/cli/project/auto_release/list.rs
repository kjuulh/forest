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
            .list_auto_release_policies(&organisation, &project)
            .await
            .context("list auto-release policies")?;

        if policies.is_empty() {
            println!("No auto-release policies found");
            return Ok(());
        }

        eprintln!("auto-release policies\n");

        for policy in policies {
            let status = if policy.enabled { "enabled" } else { "disabled" };
            println!("{} ({})", policy.name, status);

            if let Some(bp) = &policy.branch_pattern {
                println!("  branch:           {bp}");
            }
            if let Some(tp) = &policy.title_pattern {
                println!("  title:            {tp}");
            }
            if let Some(ap) = &policy.author_pattern {
                println!("  author:           {ap}");
            }
            if let Some(cmp) = &policy.commit_message_pattern {
                println!("  commit message:   {cmp}");
            }
            if let Some(stp) = &policy.source_type_pattern {
                println!("  source type:      {stp}");
            }
            if !policy.target_environments.is_empty() {
                println!("  environments:     {}", policy.target_environments.join(", "));
            }
            if !policy.target_destinations.is_empty() {
                println!("  destinations:     {}", policy.target_destinations.join(", "));
            }
            if policy.force_release {
                println!("  force:            true");
            }
            if policy.use_pipeline {
                println!("  use pipeline:     true");
            }
            println!("  id:               {}", policy.id);
            println!();
        }

        Ok(())
    }
}
