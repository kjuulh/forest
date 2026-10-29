use list::ListCommand;
use show::ShowCommand;
use version::VersionCommand;
use visibility::VisibilityCommand;

use crate::{cli::components::init::InitCommand, state::State};

pub(crate) mod generate;
pub(crate) mod init;
mod list;
pub(crate) mod publish;
mod show;
mod version;
pub(crate) mod visibility;

/// Browse and manage components in the registry.
#[derive(clap::Parser)]
#[command(subcommand_required = true)]
pub struct ComponentsCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Scaffold a new component from a template
    Init(InitCommand),
    /// Search and list components in the registry
    List(ListCommand),
    /// Show full detail (shape, tool facet, methods, platforms, versions)
    Show(ShowCommand),
    /// Print the version `forest publish` would publish from this directory:
    /// `--version`, else `FOREST_COMPONENT_VERSION`, else forest.cue. For build
    /// scripts that stamp their own binary, e.g. a `prebuilt` component.
    Version(VersionCommand),
    /// Make a component public (anyone signed in can use it) or private
    /// (members of its organisation only). Needs an admin of that organisation.
    Visibility(VisibilityCommand),
}

impl ComponentsCommand {
    pub fn is_mutation(&self) -> bool {
        matches!(self.commands, Commands::Visibility(_))
    }

    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Init(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Show(cmd) => cmd.execute(state).await,
            Commands::Version(cmd) => cmd.execute(state).await,
            Commands::Visibility(cmd) => cmd.execute(state).await,
        }
    }
}
