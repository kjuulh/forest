use global_init::GlobalInitCommand;

use crate::state::State;

mod global_init;

#[derive(clap::Parser)]
pub struct GlobalCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
#[clap(subcommand_required = true)]
enum Commands {
    Init(GlobalInitCommand),
}

impl GlobalCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Init(cmd) => cmd.execute(state).await,
        }
    }
}
