use crate::{
    cli::project::{init::InitCommand, publish::PublishCommand},
    state::State,
};

mod init;
mod publish;

#[derive(clap::Parser)]
pub struct ProjectCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    Init(InitCommand),
    Publish(PublishCommand),
}

impl ProjectCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Init(cmd) => cmd.execute(state).await,
            Commands::Publish(cmd) => cmd.execute(state).await,
        }
    }
}
