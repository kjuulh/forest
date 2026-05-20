use crate::state::State;

mod create;
mod delete;
mod evaluate;
mod list;
mod update;

#[derive(clap::Parser)]
pub struct PolicyCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Create a new deployment policy
    Create(create::CreateCommand),
    /// List policies for a project
    List(list::ListCommand),
    /// Update a policy
    Update(update::UpdateCommand),
    /// Delete a policy
    Delete(delete::DeleteCommand),
    /// Evaluate policies for a target environment (dry-run)
    Evaluate(evaluate::EvaluateCommand),
}

impl PolicyCommand {
    pub fn is_mutation(&self) -> bool {
        matches!(
            self.commands,
            Commands::Create(_) | Commands::Update(_) | Commands::Delete(_)
        )
    }

    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
            Commands::Delete(cmd) => cmd.execute(state).await,
            Commands::Evaluate(cmd) => cmd.execute(state).await,
        }
    }
}
