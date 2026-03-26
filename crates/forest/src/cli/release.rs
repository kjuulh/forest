use crate::{
    cli::release::{
        annotate::AnnotateCommand, commit::CommitCommand, create::CreateCommand,
        prepare::PrepareCommand,
    },
    state::State,
};

pub(crate) mod annotate;
pub(crate) mod commit;
mod create;
pub(crate) mod prepare;

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
    /// Prepare, annotate, and release in one step (annotation-only, no auto-release from triggers).
    Create(CreateCommand),
}

impl ReleaseCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Some(Commands::Prepare(cmd)) => cmd.execute(state).await?,
            Some(Commands::Annotate(cmd)) => cmd.execute(state).await?,
            Some(Commands::Release(cmd)) => cmd.execute(state).await?,
            Some(Commands::Create(cmd)) => cmd.execute(state).await?,
            None => {
                let cmd = self.release.as_ref().cloned().unwrap_or_default();
                cmd.execute(state).await?
            }
        }

        Ok(())
    }
}
