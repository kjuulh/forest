use anyhow::Context;

use crate::{cli::{project::pipeline::format_stages, prompts}, grpc::GrpcClientState, state::State};

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

        let pipelines = state
            .grpc_client()
            .list_release_pipelines(&organisation, &project)
            .await
            .context("list release pipelines")?;

        if pipelines.is_empty() {
            println!("No release pipelines found");
            return Ok(());
        }

        eprintln!("release pipelines\n");

        for pipeline in pipelines {
            let status = if pipeline.enabled { "enabled" } else { "disabled" };
            println!("{} ({})", pipeline.name, status);
            println!("  stages:     {}", format_stages(&pipeline.stages));
            println!("  created:    {}", pipeline.created_at);
            println!("  id:         {}", pipeline.id);
            println!();
        }

        Ok(())
    }
}
