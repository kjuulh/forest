use anyhow::Context;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct DeleteCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Trigger name to delete
    #[arg(long)]
    name: Option<String>,
}

impl DeleteCommand {
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
            None => inquire::Text::new("Trigger name:").prompt()?,
        };

        state
            .grpc_client()
            .delete_trigger(&organisation, &project, &name)
            .await
            .context("delete trigger")?;

        println!("Deleted trigger '{}'", name);

        Ok(())
    }
}
