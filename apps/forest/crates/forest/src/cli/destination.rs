use crate::{
    cli::destination::{
        create::CreateCommand, delete::DeleteCommand, list::ListCommand, reveal::RevealCommand,
        types::TypesCommand, update::UpdateCommand,
    },
    state::State,
};

mod create;
mod delete;
mod list;
mod reveal;
mod types;
mod update;

#[derive(clap::Parser)]
pub struct DestinationCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Create a deployment destination (e.g. flux/k8s, terraform, forage)
    Create(CreateCommand),
    /// Update a destination's metadata or type
    Update(UpdateCommand),
    /// Delete a destination
    Delete(DeleteCommand),
    /// List destinations in an organisation (sensitive values are hidden)
    List(ListCommand),
    /// Print one hidden metadata value for a destination
    Reveal(RevealCommand),
    /// List available destination types (the blessed kinds: flux, terraform, forage, …)
    Types(TypesCommand),
}

impl DestinationCommand {
    pub fn is_mutation(&self) -> bool {
        matches!(
            self.commands,
            Commands::Create(_) | Commands::Update(_) | Commands::Delete(_)
        )
    }

    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
            Commands::Delete(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Reveal(cmd) => cmd.execute(state).await,
            Commands::Types(cmd) => cmd.execute(state).await,
        }
    }
}
