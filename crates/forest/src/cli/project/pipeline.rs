use crate::state::State;

mod create;
mod delete;
mod list;
mod update;

#[derive(clap::Parser)]
pub struct PipelineCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Create a new release pipeline
    Create(create::CreateCommand),
    /// List release pipelines for a project
    List(list::ListCommand),
    /// Update a release pipeline
    Update(update::UpdateCommand),
    /// Delete a release pipeline
    Delete(delete::DeleteCommand),
}

impl PipelineCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
            Commands::Delete(cmd) => cmd.execute(state).await,
        }
    }
}
