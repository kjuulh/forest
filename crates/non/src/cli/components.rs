use list::ListCommand;

use crate::state::State;

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
}

impl ComponentsCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::List(list_command) => list_command.execute(state).await,
        }
    }
}
