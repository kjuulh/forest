use crate::{
    cli::release::{commit::CommitCommand, prepare::PrepareCommand, publish::AnnotateCommand},
    state::State,
};

mod commit;
mod prepare;
mod publish;

#[derive(clap::Parser)]
pub struct ReleaseCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[allow(clippy::large_enum_variant)]
#[derive(clap::Subcommand)]
pub enum Commands {
    Prepare(PrepareCommand),
    Annotate(AnnotateCommand),
    Commit(CommitCommand),
}

impl ReleaseCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Prepare(cmd) => cmd.execute(state).await?,
            Commands::Annotate(cmd) => cmd.execute(state).await?,
            Commands::Commit(cmd) => cmd.execute(state).await?,
        }

        Ok(())
    }
}
