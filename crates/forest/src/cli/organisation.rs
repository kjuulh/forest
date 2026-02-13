mod create;
mod get;
mod search;

use crate::{cli::output::OutputFormat, state::State};

#[derive(clap::Parser)]
pub struct OrganisationCommand {
    /// Output format
    #[arg(long, value_enum, default_value_t, global = true)]
    format: OutputFormat,

    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Create a new organisation
    Create(create::CreateCommand),
    /// Get an organisation by ID or name
    Get(get::GetCommand),
    /// Search organisations by name
    Search(search::SearchCommand),
}

impl OrganisationCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state, &self.format).await,
            Commands::Get(cmd) => cmd.execute(state, &self.format).await,
            Commands::Search(cmd) => cmd.execute(state, &self.format).await,
        }
    }
}
