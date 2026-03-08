use anyhow::Context;

use forest_grpc_interface::PipelineRunStageStatus;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct ReleasesCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Include recently completed releases (not just active ones).
    #[arg(long)]
    all: bool,
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

        let resp = state
            .grpc_client()
            .get_release_intent_states(&organisation, Some(&project), self.all)
            .await
            .context("get release intent states")?;

        if resp.release_intents.is_empty() {
            println!("No releases found for {organisation}/{project}");
            return Ok(());
        }

        for intent in &resp.release_intents {
            let id_short = if intent.release_intent_id.len() >= 8 {
                &intent.release_intent_id[..8]
            } else {
                &intent.release_intent_id
            };

            eprintln!("Release {id_short} ({})", intent.created_at);

            // ── Pipeline stages ────────────────────────────────
            if !intent.stages.is_empty() {
                for stage in &intent.stages {
                    let status = PipelineRunStageStatus::try_from(stage.status)
                        .unwrap_or(PipelineRunStageStatus::Unspecified);

                    let icon = match status {
                        PipelineRunStageStatus::Succeeded => "✓",
                        PipelineRunStageStatus::Active => "▶",
                        PipelineRunStageStatus::Failed
                        | PipelineRunStageStatus::Cancelled => "✗",
                        PipelineRunStageStatus::Pending => "◌",
                        _ => "•",
                    };

                    let type_str = if let Some(env) = &stage.environment {
                        format!("deploy({env})")
                    } else if let Some(dur) = stage.duration_seconds {
                        format!("wait({dur}s)")
                    } else {
                        "unknown".into()
                    };

                    let deps = if stage.depends_on.is_empty() {
                        String::new()
                    } else {
                        format!(" (after {})", stage.depends_on.join(", "))
                    };

                    println!("  {icon} {}: {type_str} [{status:?}]{deps}", stage.stage_id);

                    // Timestamps
                    let mut times = Vec::new();
                    if let Some(t) = &stage.queued_at {
                        times.push(format!("queued: {t}"));
                    }
                    if let Some(t) = &stage.started_at {
                        times.push(format!("started: {t}"));
                    }
                    if let Some(t) = &stage.completed_at {
                        times.push(format!("completed: {t}"));
                    }
                    if let Some(t) = &stage.wait_until {
                        times.push(format!("wait_until: {t}"));
                    }
                    if !times.is_empty() {
                        println!("      {}", times.join("  "));
                    }

                    if let Some(err) = &stage.error_message {
                        println!("      error: {err}");
                    }

                    // Show release steps belonging to this stage
                    for step in &intent.steps {
                        if step.stage_id.as_deref() == Some(stage.stage_id.as_str()) {
                            let step_icon = match step.status.as_str() {
                                "SUCCEEDED" => "✓",
                                "RUNNING" => "▶",
                                "ASSIGNED" => "◉",
                                "QUEUED" => "◌",
                                "FAILED" | "TIMED_OUT" | "CANCELLED" => "✗",
                                _ => "•",
                            };
                            println!(
                                "      {step_icon} {}: {} [{}]",
                                step.destination_name, step.environment, step.status
                            );
                        }
                    }
                }
            }

            // ── Non-pipeline release steps ─────────────────────
            let orphan_steps: Vec<_> = intent
                .steps
                .iter()
                .filter(|s| s.stage_id.is_none())
                .collect();

            if !orphan_steps.is_empty() {
                for step in orphan_steps {
                    let icon = match step.status.as_str() {
                        "SUCCEEDED" => "✓",
                        "RUNNING" => "▶",
                        "ASSIGNED" => "◉",
                        "QUEUED" => "◌",
                        "FAILED" | "TIMED_OUT" | "CANCELLED" => "✗",
                        _ => "•",
                    };

                    println!(
                        "  {icon} {}: {} [{}]",
                        step.destination_name, step.environment, step.status
                    );

                    let mut times = Vec::new();
                    if let Some(t) = &step.queued_at {
                        times.push(format!("queued: {t}"));
                    }
                    if let Some(t) = &step.assigned_at {
                        times.push(format!("assigned: {t}"));
                    }
                    if let Some(t) = &step.started_at {
                        times.push(format!("started: {t}"));
                    }
                    if let Some(t) = &step.completed_at {
                        times.push(format!("completed: {t}"));
                    }
                    if !times.is_empty() {
                        println!("    {}", times.join("  "));
                    }

                    if let Some(err) = &step.error_message {
                        println!("    error: {err}");
                    }
                }
            }

            println!();
        }

        Ok(())
    }
}
