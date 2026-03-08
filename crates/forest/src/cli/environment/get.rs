use anyhow::Context;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct GetCommand {
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    #[arg(long)]
    name: Option<String>,
}

impl GetCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let organisation = match &self.organisation {
            Some(o) => o.clone(),
            None => prompts::select_organisation(state).await?,
        };

        let name = match &self.name {
            Some(n) => n.clone(),
            None => inquire::Text::new("Environment name:").prompt()?,
        };

        let env = state
            .grpc_client()
            .get_environment(&organisation, &name)
            .await
            .context("get environment")?;

        println!("{}", env.name);
        println!("  id:           {}", env.id);
        println!("  organisation: {}", env.organisation);
        if let Some(desc) = &env.description {
            println!("  description:  {desc}");
        }
        println!("  sort order:   {}", env.sort_order);
        println!("  created at:   {}", env.created_at);

        Ok(())
    }
}
