mod create;
mod get;
mod member;
mod oauth_app;
mod search;
mod visibility;

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
    /// Manage OAuth applications ("Sign in with Forest")
    #[command(alias = "oauth-apps", alias = "oauth")]
    OauthApp(oauth_app::OAuthAppCommand),
    /// Make every component the organisation publishes public (read-only to
    /// anyone signed in) or private. Needs an admin of the organisation.
    Visibility(visibility::VisibilityCommand),
}

impl OrganisationCommand {
    pub fn is_mutation(&self) -> bool {
        match &self.commands {
            Commands::Create(_) | Commands::Visibility(_) => true,
            Commands::Show(_) | Commands::Search(_) => false,
            Commands::Member(c) => c.is_mutation(),
            Commands::OauthApp(c) => c.is_mutation(),
        }
    }

    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let format = state.config.format;
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state, &format).await,
            Commands::Show(cmd) => cmd.execute(state, &format).await,
            Commands::Search(cmd) => cmd.execute(state, &format).await,
            Commands::Member(cmd) => cmd.execute(state, &format).await,
            Commands::OauthApp(cmd) => cmd.execute(state, &format).await,
            Commands::Visibility(cmd) => cmd.execute(state).await,
        }
    }
}
