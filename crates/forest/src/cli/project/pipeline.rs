use std::collections::HashMap;

use anyhow::Context;
use forest_grpc_interface::{pipeline_stage, DeployStageConfig, PipelineStage, WaitStageConfig};
use serde::Deserialize;

use crate::state::State;

mod create;
mod delete;
mod list;
mod update;

#[derive(clap::Parser)]
pub struct PipelineCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Create a new release pipeline
    Create(create::CreateCommand),
    /// List release pipelines for a project
    List(list::ListCommand),
    /// Update a release pipeline
    Update(update::UpdateCommand),
    /// Delete a release pipeline
    Delete(delete::DeleteCommand),
}

impl PipelineCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
            Commands::Delete(cmd) => cmd.execute(state).await,
        }
    }
}

// ── JSON -> proto stage conversion ───────────────────────────────────

/// Intermediate JSON format for stages (matches the DB/domain model).
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JsonStageConfig {
    Deploy { environment: String },
    Wait { duration_seconds: i64 },
}

#[derive(Deserialize)]
struct JsonStageDefinition {
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(flatten)]
    config: JsonStageConfig,
}

/// Parse a JSON string (map of stage-id -> stage definition) into proto PipelineStage messages.
pub fn parse_stages_from_json(json: &str) -> anyhow::Result<Vec<PipelineStage>> {
    let stages: HashMap<String, JsonStageDefinition> =
        serde_json::from_str(json).context("invalid stages JSON")?;

    let proto_stages = stages
        .into_iter()
        .map(|(id, def)| {
            let config = match def.config {
                JsonStageConfig::Deploy { environment } => {
                    pipeline_stage::Config::Deploy(DeployStageConfig { environment })
                }
                JsonStageConfig::Wait { duration_seconds } => {
                    pipeline_stage::Config::Wait(WaitStageConfig { duration_seconds })
                }
            };

            PipelineStage {
                id,
                depends_on: def.depends_on,
                config: Some(config),
            }
        })
        .collect();

    Ok(proto_stages)
}

/// Format proto PipelineStage messages for display.
pub fn format_stages(stages: &[PipelineStage]) -> String {
    if stages.is_empty() {
        return "(no stages)".to_string();
    }

    let mut parts = Vec::new();
    for s in stages {
        let type_str = match &s.config {
            Some(pipeline_stage::Config::Deploy(c)) => {
                format!("deploy({})", c.environment)
            }
            Some(pipeline_stage::Config::Wait(c)) => {
                format!("wait({}s)", c.duration_seconds)
            }
            None => "unknown".to_string(),
        };

        let deps = if s.depends_on.is_empty() {
            String::new()
        } else {
            format!(" -> [{}]", s.depends_on.join(", "))
        };

        parts.push(format!("{}: {}{}", s.id, type_str, deps));
    }
    parts.join(", ")
}
