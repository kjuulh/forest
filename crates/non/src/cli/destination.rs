use crate::{cli::destination::create::CreateCommand, state::State};

mod create;

#[derive(clap::Parser)]
pub struct DestinationCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    Create(CreateCommand),
}

impl DestinationCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state).await,
        }
    }
}
