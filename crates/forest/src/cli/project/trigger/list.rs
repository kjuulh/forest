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

        let triggers = state
            .grpc_client()
            .list_triggers(&organisation, &project)
            .await
            .context("list triggers")?;

        if triggers.is_empty() {
            eprintln!("No triggers found");
            return Ok(());
        }

        eprintln!("triggers\n");

        for trigger in triggers {
            let status = if trigger.enabled {
                "enabled"
            } else {
                "disabled"
            };
            println!("{} ({})", trigger.name, status);

            if let Some(bp) = &trigger.branch_pattern {
                println!("  branch:           {bp}");
            }
            if let Some(tp) = &trigger.title_pattern {
                println!("  title:            {tp}");
            }
            if let Some(ap) = &trigger.author_pattern {
                println!("  author:           {ap}");
            }
            if let Some(cmp) = &trigger.commit_message_pattern {
                println!("  commit message:   {cmp}");
            }
            if let Some(stp) = &trigger.source_type_pattern {
                println!("  source type:      {stp}");
            }
            if !trigger.target_environments.is_empty() {
                println!(
                    "  environments:     {}",
                    trigger.target_environments.join(", ")
                );
            }
            if !trigger.target_destinations.is_empty() {
                println!(
                    "  destinations:     {}",
                    trigger.target_destinations.join(", ")
                );
            }
            if trigger.force_release {
                println!("  force:            true");
            }
            if trigger.use_pipeline {
                println!("  use pipeline:     true");
            }
            println!("  id:               {}", trigger.id);
            println!();
        }

        Ok(())
    }
}
