use crate::state::State;

mod login;
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
    /// Create a new account
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
