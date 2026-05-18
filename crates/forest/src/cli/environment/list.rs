use anyhow::Context;

use crate::{cli::prompts, grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct ListCommand {
    #[arg(long, short = 'o', visible_alias = "org")]
    organisation: Option<String>,
}

impl ListCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let organisation = match &self.organisation {
            Some(o) => o.clone(),
            None => prompts::select_organisation(state).await?,
        };

        let envs = state
            .grpc_client()
            .list_environments(&organisation)
            .await
            .context("list environments")?;

        if envs.is_empty() {
            eprintln!("No environments found");
            return Ok(());
        }

        eprintln!("environments\n");

        for env in envs {
            println!("{}", env.name);
            if let Some(desc) = &env.description {
                println!("  description: {desc}");
            }
            println!("  sort order:  {}", env.sort_order);
            println!("  id:          {}", env.id);
        }

        Ok(())
    }
}
