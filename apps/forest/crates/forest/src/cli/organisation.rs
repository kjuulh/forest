mod create;
mod get;
mod member;
mod search;

use crate::state::State;

#[derive(clap::Parser)]
pub struct OrganisationCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Create a new organisation
    Create(create::CreateCommand),
    /// Show full details of an organisation by ID or name
    #[command(alias = "get")]
    Show(get::GetCommand),
    /// Search organisations by name
    Search(search::SearchCommand),
    /// Manage organisation members
    Member(member::MemberCommand),
}

impl OrganisationCommand {
    pub fn is_mutation(&self) -> bool {
        match &self.commands {
            Commands::Create(_) => true,
            Commands::Show(_) | Commands::Search(_) => false,
            Commands::Member(c) => c.is_mutation(),
        }
    }

    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        // The --format flag is hoisted to global Config; subcommands read it
        // from state.config.format.
        let format = state.config.format;
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state, &format).await,
            Commands::Show(cmd) => cmd.execute(state, &format).await,
            Commands::Search(cmd) => cmd.execute(state, &format).await,
            Commands::Member(cmd) => cmd.execute(state, &format).await,
        }
    }
}
