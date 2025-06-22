use create_namespace::CreateNamespaceCommand;
use deploy_components::DeployComponentCommand;

use crate::state::State;

mod create_namespace;
mod deploy_components;

#[derive(clap::Parser)]
#[command(subcommand_required = true, hide(true))]
pub struct AdminCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    CreateNamespace(CreateNamespaceCommand),
    DeployComponent(DeployComponentCommand),
}

impl Commands {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match self {
            Commands::CreateNamespace(create_namespace_command) => {
                create_namespace_command.execute(state).await
            }
            Commands::DeployComponent(cmd) => cmd.execute(state).await,
        }
    }
}

impl AdminCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        self.commands.execute(state).await?;

        Ok(())
    }
}
