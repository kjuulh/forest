use crate::{
    cli::environment::{
        create::CreateCommand, delete::DeleteCommand, get::GetCommand, list::ListCommand,
        update::UpdateCommand,
    },
    state::State,
};

mod create;
mod delete;
mod get;
mod list;
mod update;

#[derive(clap::Parser)]
pub struct EnvironmentCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// List environments for an organisation
    List(ListCommand),
    /// Create a new environment
    Create(CreateCommand),
    /// Show details of an environment
    #[command(alias = "get")]
    Show(GetCommand),
    /// Update an environment
    Update(UpdateCommand),
    /// Delete an environment
    Delete(DeleteCommand),
}

impl EnvironmentCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Create(cmd) => cmd.execute(state).await,
            Commands::Show(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
            Commands::Delete(cmd) => cmd.execute(state).await,
        }
    }
}
