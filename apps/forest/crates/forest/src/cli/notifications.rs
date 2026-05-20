mod format;
mod listen;
mod list;
mod preferences;

use crate::state::State;

#[derive(clap::Parser)]
pub struct NotificationsCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Listen for real-time notifications
    Listen(listen::ListenCommand),
    /// List recent notifications
    List(list::ListCommand),
    /// Manage notification preferences
    Preferences(preferences::PreferencesCommand),
}

impl NotificationsCommand {
    pub fn is_mutation(&self) -> bool {
        match &self.commands {
            Commands::Listen(_) | Commands::List(_) => false,
            Commands::Preferences(c) => c.is_mutation(),
        }
    }

    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Listen(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Preferences(cmd) => cmd.execute(state).await,
        }
    }
}
