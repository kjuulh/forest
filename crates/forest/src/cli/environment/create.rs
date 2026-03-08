use anyhow::Context;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct CreateCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long)]
    name: Option<String>,

    #[arg(long)]
    description: Option<String>,

    #[arg(long)]
    sort_order: Option<i32>,
}

impl CreateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let organisation = match &self.organisation {
            Some(o) => o.clone(),
            None => prompts::select_organisation(state).await?,
        };

        let name = match &self.name {
            Some(n) => n.clone(),
            None => inquire::Text::new("Environment name:").prompt()?,
        };

        let description = match &self.description {
            Some(d) => Some(d.clone()),
            None => inquire::Text::new("Description (optional):")
                .prompt_skippable()?
                .filter(|s| !s.is_empty()),
        };

        let sort_order = match self.sort_order {
            Some(s) => s,
            None => inquire::Text::new("Sort order:")
                .with_default("0")
                .prompt()?
                .parse()
                .context("sort order must be a number")?,
        };

        let env = state
            .grpc_client()
            .create_environment(&organisation, &name, description.as_deref(), sort_order)
            .await
            .context("create environment")?;

        println!("Created environment '{}' ({})", env.name, env.id);

        Ok(())
    }
}
