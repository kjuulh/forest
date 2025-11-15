use crate::{cli::deploy::prepare::PrepareCommand, state::State};

mod prepare;

#[derive(clap::Parser)]
pub struct DeployCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
pub enum Commands {
    Prepare(PrepareCommand),
}

impl DeployCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Prepare(cmd) => cmd.execute(state).await?,
        }

        Ok(())
    }
}
