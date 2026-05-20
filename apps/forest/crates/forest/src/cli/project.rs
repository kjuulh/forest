use crate::{
    cli::project::{
        create::CreateCommand, init::InitCommand, list::ListCommand,
        pipeline::PipelineCommand, policy::PolicyCommand, publish::PublishCommand,
        releases::ReleasesCommand, trigger::TriggerCommand,
    },
    state::State,
};

mod create;
mod init;
mod list;
mod pipeline;
mod policy;
mod publish;
pub(crate) mod releases;
mod trigger;

#[derive(clap::Parser)]
pub struct ProjectCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Create a project in an organisation
    Create(CreateCommand),
    /// Scaffold project files locally (forest.cue + cue.mod) for a fresh checkout
    Init(InitCommand),
    /// Publish the current project's metadata to the registry (org + name + visibility)
    Publish(Box<PublishCommand>),
    /// List projects (filterable by organisation)
    List(ListCommand),
    /// Show current release state per destination for a project
    Releases(ReleasesCommand),
    /// Manage release triggers for a project
    Trigger(TriggerCommand),
    /// Manage deployment policies (guardrails) for a project
    Policy(PolicyCommand),
    /// Manage release pipelines for a project
    Pipeline(PipelineCommand),
}

impl ProjectCommand {
    pub fn is_mutation(&self) -> bool {
        match &self.commands {
            Commands::Create(_) | Commands::Init(_) | Commands::Publish(_) => true,
            Commands::List(_) | Commands::Releases(_) => false,
            Commands::Trigger(c) => c.is_mutation(),
            Commands::Policy(c) => c.is_mutation(),
            Commands::Pipeline(c) => c.is_mutation(),
        }
    }

    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Create(cmd) => cmd.execute(state).await,
            Commands::Init(cmd) => cmd.execute(state).await,
            Commands::Publish(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Releases(cmd) => cmd.execute(state).await,
            Commands::Trigger(cmd) => cmd.execute(state).await,
            Commands::Policy(cmd) => cmd.execute(state).await,
            Commands::Pipeline(cmd) => cmd.execute(state).await,
        }
    }
}
