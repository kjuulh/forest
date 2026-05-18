use anyhow::Context;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

use super::parse_stages_from_json;

#[derive(clap::Parser)]
pub struct UpdateCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Pipeline name to update
    #[arg(long)]
    name: Option<String>,

    /// Enable or disable the pipeline
    #[arg(long)]
    enabled: Option<bool>,

    /// Replace pipeline stages with new JSON
    #[arg(long)]
    stages_json: Option<String>,

    /// Read replacement stages JSON from a file
    #[arg(long, conflicts_with = "stages_json")]
    stages_file: Option<String>,
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
            None => inquire::Text::new("Pipeline name:").prompt()?,
        };

        let stages = if let Some(path) = &self.stages_file {
            let json = std::fs::read_to_string(path)
                .context(format!("read stages file: {path}"))?;
            Some(parse_stages_from_json(&json)?)
        } else if let Some(json) = &self.stages_json {
            Some(parse_stages_from_json(json)?)
        } else {
            None
        };

        let pipeline = state
            .grpc_client()
            .update_release_pipeline(
                &organisation,
                &project,
                &name,
                self.enabled,
                stages,
            )
            .await
            .context("update release pipeline")?;

        let status = if pipeline.enabled { "enabled" } else { "disabled" };
        eprintln!("Updated release pipeline '{}' ({})", pipeline.name, status);

        Ok(())
    }
}
