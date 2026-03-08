use anyhow::Context;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct CreateCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Pipeline name (unique per project)
    #[arg(long)]
    name: Option<String>,

    /// Pipeline stages as JSON (DAG definition)
    #[arg(long)]
    stages_json: Option<String>,

    /// Read stages JSON from a file instead of --stages-json
    #[arg(long, conflicts_with = "stages_json")]
    stages_file: Option<String>,
}

impl CreateCommand {
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
            std::fs::read_to_string(path)
                .context(format!("read stages file: {path}"))?
        } else if let Some(json) = &self.stages_json {
            json.clone()
        } else {
            inquire::Text::new("Stages JSON:").prompt()?
        };

        // Validate JSON locally before sending
        serde_json::from_str::<serde_json::Value>(&stages_json)
            .context("invalid JSON for stages")?;

        let pipeline = state
            .grpc_client()
            .create_release_pipeline(&organisation, &project, &name, &stages_json)
            .await
            .context("create release pipeline")?;

        println!("Created release pipeline '{}'", pipeline.name);
        let status = if pipeline.enabled { "enabled" } else { "disabled" };
        println!("  status:  {status}");
        println!("  id:      {}", pipeline.id);

        Ok(())
    }
}
