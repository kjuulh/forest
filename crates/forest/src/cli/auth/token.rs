use anyhow::Context;

use crate::{state::State, user_state::UserStateLoaderState};

mod create;
mod delete;
mod list;

/// Resolve the user id for personal-access-token operations. Falls back to the
/// currently logged-in user when `--user-id` is not provided.
pub(super) async fn resolve_user_id(
    state: &State,
    user_id: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(id) = user_id {
        return Ok(id.to_string());
    }
    let current = state
        .user_state()
        .get_state()
        .await?
        .context("not logged in — run `forest auth login` or pass --user-id")?;
    Ok(current.user_id)
}

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
            _ => {
                let state = state
                    .user_state()
                    .get_state()
                    .await?
                    .context("user not logged in or expired")?;

                println!("{}", state.access_token);

                Ok(())
            }
        }
    }
}
