use list::ListCommand;

use crate::{
    cli::components::init::InitCommand,
    state::State,
};

pub(crate) mod build;
pub(crate) mod generate;
pub(crate) mod init;
mod list;
pub(crate) mod publish;

/// Browse and manage components in the registry.
#[derive(clap::Parser)]
#[command(subcommand_required = true)]
pub struct ComponentsCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Scaffold a new component from a template
    Init(InitCommand),
    /// Search and list components in the registry
    List(ListCommand),
}

impl ComponentsCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Init(cmd) => cmd.execute(state).await,
            Commands::List(list_command) => list_command.execute(state).await,
        }
    }
}
