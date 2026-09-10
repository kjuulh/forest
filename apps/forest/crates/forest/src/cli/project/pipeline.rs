use std::collections::HashMap;

use anyhow::Context;
use forest_grpc_interface::{GateStageConfig, GateTimeoutBehaviour, HealthStatus, SignalRequirement, 
    DeployStageConfig, PipelineStage, PlanStageConfig, WaitStageConfig, pipeline_stage,
};
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
    pub fn is_mutation(&self) -> bool {
        !matches!(self.commands, Commands::List(_))
    }

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
    Deploy {
        environment: String,
    },
    Wait {
        duration_seconds: i64,
    },
    Plan {
        environment: String,
        #[serde(default)]
        auto_approve: bool,
    },
    /// Wait for evidence rather than a duration — see forest#252.
    ///
    ///     "await-demo": {
    ///       "type": "gate",
    ///       "requires": [{"signal": "rollout", "in": ["HEALTHY"]}],
    ///       "timeout_seconds": 600,
    ///       "depends_on": ["deploy-demo"]
    ///     }
    Gate {
        requires: Vec<JsonSignalRequirement>,
        timeout_seconds: i64,
        #[serde(default)]
        on_timeout: JsonGateTimeout,
    },
}

#[derive(Deserialize)]
pub struct JsonSignalRequirement {
    signal: String,
    /// Any one of these satisfies it. Omitted means `["HEALTHY"]` — treating
    /// an empty list as "any status" would open the gate on UNHEALTHY.
    #[serde(default, rename = "in")]
    accept: Vec<String>,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonGateTimeout {
    #[default]
    Fail,
    Proceed,
}

#[derive(Deserialize)]
struct JsonStageDefinition {
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(flatten)]
    config: JsonStageConfig,
}

/// The status names a gate requirement may use, as the CLI accepts them.
///
/// An unrecognised name becomes UNSPECIFIED, which the server rejects with a
/// message naming the valid ones — better than the CLI guessing at a default
/// and building a gate that waits for something nobody can report.
fn health_status_from_str(s: &str) -> i32 {
    match s.to_ascii_uppercase().as_str() {
        "HEALTHY" => HealthStatus::Healthy as i32,
        "PROGRESSING" => HealthStatus::Progressing as i32,
        "DEGRADED" => HealthStatus::Degraded as i32,
        "UNHEALTHY" => HealthStatus::Unhealthy as i32,
        "MISSING" => HealthStatus::Missing as i32,
        _ => HealthStatus::Unspecified as i32,
    }
}

fn health_status_str(v: i32) -> &'static str {
    match HealthStatus::try_from(v) {
        Ok(HealthStatus::Healthy) => "HEALTHY",
        Ok(HealthStatus::Progressing) => "PROGRESSING",
        Ok(HealthStatus::Degraded) => "DEGRADED",
        Ok(HealthStatus::Unhealthy) => "UNHEALTHY",
        Ok(HealthStatus::Missing) => "MISSING",
        _ => "UNSPECIFIED",
    }
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
                JsonStageConfig::Plan {
                    environment,
                    auto_approve,
                } => pipeline_stage::Config::Plan(PlanStageConfig {
                    environment,
                    auto_approve,
                }),
                JsonStageConfig::Gate {
                    requires,
                    timeout_seconds,
                    on_timeout,
                } => pipeline_stage::Config::Gate(GateStageConfig {
                    requires: requires
                        .into_iter()
                        .map(|r| SignalRequirement {
                            signal: r.signal,
                            accept: r.accept.iter().map(|s| health_status_from_str(s)).collect(),
                        })
                        .collect(),
                    timeout_seconds,
                    on_timeout: match on_timeout {
                        JsonGateTimeout::Fail => GateTimeoutBehaviour::Fail as i32,
                        JsonGateTimeout::Proceed => GateTimeoutBehaviour::Proceed as i32,
                    },
                }),
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
            Some(pipeline_stage::Config::Plan(c)) => {
                let auto = if c.auto_approve { ", auto" } else { "" };
                format!("plan({}{})", c.environment, auto)
            }
            Some(pipeline_stage::Config::Gate(c)) => {
                let reqs: Vec<String> = c
                    .requires
                    .iter()
                    .map(|r| {
                        let accepted: Vec<&str> =
                            r.accept.iter().map(|s| health_status_str(*s)).collect();
                        let accepted = if accepted.is_empty() {
                            "HEALTHY".to_string()
                        } else {
                            accepted.join("|")
                        };
                        format!("{}={}", r.signal, accepted)
                    })
                    .collect();
                format!("gate({}, {}s)", reqs.join(" "), c.timeout_seconds)
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
