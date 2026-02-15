use list::ListCommand;

use crate::{
    cli::components::{build::BuildCommand, generate::GenerateCommand},
    state::State,
};

mod build;
mod generate;
mod list;

#[derive(clap::Parser)]
#[command(subcommand_required = true)]
pub struct ComponentsCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    List(ListCommand),
    Generate(GenerateCommand),
    Build(BuildCommand),
}

impl ComponentsCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::List(list_command) => list_command.execute(state).await,
            Commands::Generate(cmd) => cmd.execute(state).await,
            Commands::Build(cmd) => cmd.execute(state).await,
        }
    }
}
