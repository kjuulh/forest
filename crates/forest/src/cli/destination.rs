use crate::{
    cli::destination::{create::CreateCommand, update::UpdateCommand},
    state::State,
};

mod create;
mod update;

#[derive(clap::Parser)]
pub struct DestinationCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    Create(CreateCommand),
    Update(UpdateCommand),
}

impl DestinationCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
        }
    }
}
