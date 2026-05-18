use anyhow::Context;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct CreateCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    /// Project name
    #[arg(long, short = 'p')]
    project: Option<String>,
}

impl CreateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let organisation = match &self.organisation {
            Some(o) => o.clone(),
            None => prompts::select_organisation(state).await?,
        };

        let project = match &self.project {
            Some(p) => p.clone(),
            None => inquire::Text::new("Project name:").prompt()?,
        };

        state
            .grpc_client()
            .create_project(&organisation, &project)
            .await
            .context("create project")?;

        eprintln!("Created project '{organisation}/{project}'");

        Ok(())
    }
}
