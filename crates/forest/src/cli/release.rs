use crate::{
    cli::release::{annotate::AnnotateCommand, commit::CommitCommand, prepare::PrepareCommand},
    state::State,
};

mod annotate;
mod commit;
mod prepare;

#[derive(clap::Parser)]
#[clap(subcommand_required = false, args_conflicts_with_subcommands = true)]
pub struct ReleaseCommand {
    #[command(subcommand)]
    commands: Option<Commands>,

    #[command(flatten)]
    release: Option<CommitCommand>,
}

#[allow(clippy::large_enum_variant)]
#[derive(clap::Subcommand)]
pub enum Commands {
    Prepare(PrepareCommand),
    Annotate(AnnotateCommand),
    Release(CommitCommand),
}

impl ReleaseCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Some(Commands::Prepare(cmd)) => cmd.execute(state).await?,
            Some(Commands::Annotate(cmd)) => cmd.execute(state).await?,
            Some(Commands::Release(cmd)) => cmd.execute(state).await?,
            None => {
                let cmd = self.release.as_ref().unwrap();
                cmd.execute(state).await?
            }
        }

        Ok(())
    }
}
