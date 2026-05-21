use crate::state::State;

mod login;
mod login_web;
mod logout;
mod register;
mod status;
mod token;

#[derive(clap::Parser)]
pub struct AuthCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Create a new account.
    ///
    /// Password requirements: at least 12 characters, containing at least
    /// one lowercase letter and one uppercase letter. Set `FOREST_PASSWORD`
    /// to bypass the interactive prompt (useful in CI / non-TTY contexts).
    Register(register::RegisterCommand),
    /// Authenticate with the forest server
    Login(login::LoginCommand),
    /// Log out of the current session
    Logout(logout::LogoutCommand),
    /// Show current authentication status
    Status(status::StatusCommand),
    /// Manage personal access tokens
    Token(token::TokenCommand),
}

impl AuthCommand {
    /// True for subcommands that change server-side or persisted auth
    /// state. `Status` is purely read-only.
    pub fn is_mutation(&self) -> bool {
        !matches!(self.commands, Commands::Status(_))
    }

    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Register(cmd) => cmd.execute(state).await,
            Commands::Login(cmd) => cmd.execute(state).await,
            Commands::Logout(cmd) => cmd.execute(state).await,
            Commands::Status(cmd) => cmd.execute(state).await,
            Commands::Token(cmd) => cmd.execute(state).await,
        }
    }
}
