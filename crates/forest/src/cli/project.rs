use crate::{
    cli::project::{
        auto_release::AutoReleaseCommand, init::InitCommand, list::ListCommand,
        pipeline::PipelineCommand, publish::PublishCommand, releases::ReleasesCommand,
    },
    state::State,
};

mod auto_release;
mod init;
mod list;
mod pipeline;
mod publish;
pub(crate) mod releases;

#[derive(clap::Parser)]
pub struct ProjectCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    Init(InitCommand),
    Publish(Box<PublishCommand>),
    List(ListCommand),
    /// Show current release state per destination for a project
    Releases(ReleasesCommand),
    /// Manage auto-release policies for a project
    AutoRelease(AutoReleaseCommand),
    /// Manage release pipelines for a project
    Pipeline(PipelineCommand),
}

impl ProjectCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Init(cmd) => cmd.execute(state).await,
            Commands::Publish(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Releases(cmd) => cmd.execute(state).await,
            Commands::AutoRelease(cmd) => cmd.execute(state).await,
            Commands::Pipeline(cmd) => cmd.execute(state).await,
        }
    }
}
