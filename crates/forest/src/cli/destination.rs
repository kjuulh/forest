use crate::{
    cli::destination::{
        create::CreateCommand, delete::DeleteCommand, list::ListCommand, types::TypesCommand,
        update::UpdateCommand,
    },
    state::State,
};

mod create;
mod delete;
mod list;
mod types;
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
    Delete(DeleteCommand),
    List(ListCommand),
    /// List available destination types
    Types(TypesCommand),
}

impl DestinationCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
            Commands::Delete(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Types(cmd) => cmd.execute(state).await,
        }
    }
}
