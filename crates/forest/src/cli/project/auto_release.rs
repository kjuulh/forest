use crate::state::State;

mod create;
mod delete;
mod list;
mod update;

#[derive(clap::Parser)]
pub struct AutoReleaseCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Create a new auto-release policy
    Create(create::CreateCommand),
    /// List auto-release policies for a project
    List(list::ListCommand),
    /// Update an auto-release policy
    Update(update::UpdateCommand),
    /// Delete an auto-release policy
    Delete(delete::DeleteCommand),
}

impl AutoReleaseCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
            Commands::Delete(cmd) => cmd.execute(state).await,
        }
    }
}
