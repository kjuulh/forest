use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct UpdateCommand {
    #[arg(long)]
    id: Option<String>,

    #[arg(long)]
    description: Option<String>,

    #[arg(long)]
    sort_order: Option<i32>,
}

impl UpdateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let id = match &self.id {
            Some(id) => id.clone(),
            None => inquire::Text::new("Environment ID:").prompt()?,
        };

        let description = match &self.description {
            Some(d) => Some(d.clone()),
            None => inquire::Text::new("Description (optional, leave empty to skip):")
                .prompt_skippable()?
                .filter(|s| !s.is_empty()),
        };

        let sort_order = match self.sort_order {
            Some(s) => Some(s),
            None => {
                let input = inquire::Text::new("Sort order (optional, leave empty to skip):")
                    .prompt_skippable()?
                    .filter(|s| !s.is_empty());
                match input {
                    Some(s) => Some(s.parse().context("sort order must be a number")?),
                    None => None,
                }
            }
        };

        let env = state
            .grpc_client()
            .update_environment(&id, description.as_deref(), sort_order)
            .await
            .context("update environment")?;

        println!("Updated environment '{}' ({})", env.name, env.id);

        Ok(())
    }
}
