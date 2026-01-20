use crate::{
    cli::destination::{create::CreateCommand, list::ListCommand, update::UpdateCommand},
    state::State,
};

mod create;
mod list;
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
    List(ListCommand),
}

impl DestinationCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
        }
    }
}
