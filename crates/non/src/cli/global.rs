use global_add::GlobalAddCommand;
use global_init::GlobalInitCommand;
use global_set::GlobalSetCommand;

use crate::state::State;

mod global_add;
mod global_init;
mod global_set;

#[derive(clap::Parser)]
pub struct GlobalCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
#[clap(subcommand_required = true)]
enum Commands {
    Init(GlobalInitCommand),
    Set(GlobalSetCommand),
    Add(GlobalAddCommand),
}

impl GlobalCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Init(cmd) => cmd.execute(state).await,
            Commands::Set(cmd) => cmd.execute(state).await,
            Commands::Add(cmd) => cmd.execute(state).await,
        }
    }
}
