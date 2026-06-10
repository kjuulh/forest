use deploy_components::DeployComponentCommand;
use get_component::GetComponentCommand;
use unpublish::UnpublishCommand;

use crate::state::State;

mod deploy_components;
mod get_component;
mod unpublish;

#[derive(clap::Parser)]
#[command(subcommand_required = true, hide(true))]
pub struct AdminCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    DeployComponent(DeployComponentCommand),
    GetComponent(GetComponentCommand),
    /// Remove a previously-published version. TASKS/025.
    Unpublish(UnpublishCommand),
}

impl Commands {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match self {
            Commands::DeployComponent(cmd) => cmd.execute(state).await,
            Commands::GetComponent(get_component_command) => {
                get_component_command.execute(state).await
            }
            Commands::Unpublish(cmd) => cmd.execute(state).await,
        }
    }
}

impl AdminCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        self.commands.execute(state).await?;

        Ok(())
    }
}
