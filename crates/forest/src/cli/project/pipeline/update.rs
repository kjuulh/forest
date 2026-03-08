use anyhow::Context;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

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

        let stages_json = if let Some(path) = &self.stages_file {
            let json = std::fs::read_to_string(path)
                .context(format!("read stages file: {path}"))?;
            serde_json::from_str::<serde_json::Value>(&json)
                .context("invalid JSON for stages")?;
            Some(json)
        } else {
            self.stages_json.clone()
        };

        let pipeline = state
            .grpc_client()
            .update_release_pipeline(
                &organisation,
                &project,
                &name,
                self.enabled,
                stages_json,
            )
            .await
            .context("update release pipeline")?;

        let status = if pipeline.enabled { "enabled" } else { "disabled" };
        println!("Updated release pipeline '{}' ({})", pipeline.name, status);

        Ok(())
    }
}
