use crate::state::State;

mod create;
mod delete;
mod list;
mod update;

#[derive(clap::Parser)]
pub struct TriggerCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Create a new trigger
    Create(create::CreateCommand),
    /// List triggers for a project
    List(list::ListCommand),
    /// Update a trigger
    Update(update::UpdateCommand),
    /// Delete a trigger
    Delete(delete::DeleteCommand),
}

impl TriggerCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
            Commands::Delete(cmd) => cmd.execute(state).await,
        }
    }
}
