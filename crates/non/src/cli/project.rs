use crate::{
    cli::project::{
        init::InitCommand, list::ListCommand, publish::PublishCommand, release::ReleaseCommand,
    },
    state::State,
};

mod init;
mod list;
mod publish;
mod release;

#[derive(clap::Parser)]
pub struct ProjectCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    Init(InitCommand),
    Publish(PublishCommand),
    Release(ReleaseCommand),
    List(ListCommand),
}

impl ProjectCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Init(cmd) => cmd.execute(state).await,
            Commands::Publish(cmd) => cmd.execute(state).await,
            Commands::Release(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
        }
    }
}
