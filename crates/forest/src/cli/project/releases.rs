use anyhow::Context;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct ReleasesCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,
}

impl ReleasesCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let organisation = match &self.organisation {
            Some(o) => o.clone(),
            None => prompts::select_organisation(state).await?,
        };

        let project = match &self.project {
            Some(p) => p.clone(),
            None => prompts::select_project(state, &organisation).await?,
        };

        let rows = state
            .grpc_client()
            .get_destination_states(&organisation, Some(&project))
            .await
            .context("get destination states")?;

        if rows.is_empty() {
            println!("No releases found for {organisation}/{project}");
            return Ok(());
        }

        eprintln!("{organisation}/{project}\n");

        let mut current_env = String::new();

        for row in &rows {
            if row.environment != current_env {
                if !current_env.is_empty() {
                    println!();
                }
                current_env.clone_from(&row.environment);
                eprintln!("── {} ──", current_env);
            }

            let status = row.status.as_deref().unwrap_or("—");
            let dest = &row.destination_name;

            if let Some(pos) = row.queue_position {
                // In-flight / queued release
                let prefix = match status {
                    "RUNNING" => "▶",
                    "ASSIGNED" => "◉",
                    _ => "◌", // QUEUED
                };
                println!("  {prefix} #{pos} {dest}: {status}");
            } else {
                // Current (latest completed) release
                let prefix = match status {
                    "SUCCEEDED" => "✓",
                    "FAILED" | "TIMED_OUT" => "✗",
                    _ => "•",
                };
                println!("  {prefix} {dest}: {status}");
                if let Some(err) = &row.error_message {
                    println!("    error: {err}");
                }
                if let Some(completed) = &row.completed_at {
                    println!("    completed: {completed}");
                }
            }
        }

        Ok(())
    }
}
