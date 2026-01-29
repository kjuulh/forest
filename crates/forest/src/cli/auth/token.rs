use crate::state::State;

mod create;
mod delete;
mod list;

#[derive(clap::Parser)]
pub struct TokenCommand {
    #[command(subcommand)]
    commands: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Create a new personal access token
    Create(create::CreateTokenCommand),
    /// List personal access tokens
    List(list::ListTokensCommand),
    /// Delete a personal access token
    Delete(delete::DeleteTokenCommand),
}

impl TokenCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Some(commands) => match &commands {
                Commands::Create(cmd) => cmd.execute(state).await,
                Commands::List(cmd) => cmd.execute(state).await,
                Commands::Delete(cmd) => cmd.execute(state).await,
            },
            _ => todo!("return the personal access token"),
        }
    }
}
