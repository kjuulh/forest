use admin::AdminCommand;
use clap::{Parser, Subcommand};
use components::ComponentsCommand;
use global::GlobalCommand;
use init::InitCommand;
use notmad::{Component, MadError};
use publish::PublishCommand;
use run::RunCommand;
use shell::ShellCommand;
use template::TemplateCommand;
use tmp::TmpCommand;
use tokio_util::sync::CancellationToken;

use crate::{cli::project::ProjectCommand, state::State};

mod admin;
mod components;
mod global;
mod init;
mod project;
mod publish;
mod run;
mod shell;
mod template;
mod tmp;

#[derive(Parser)]
#[command(author, version, about, long_about = None, subcommand_required = true)]
struct Command {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Init(InitCommand),
    Components(ComponentsCommand),
    Admin(AdminCommand),
    Run(RunCommand),
    Template(TemplateCommand),
    Publish(PublishCommand),
    Global(GlobalCommand),
    Shell(ShellCommand),
    Tmp(TmpCommand),
    Project(ProjectCommand),
}

pub async fn execute() -> anyhow::Result<()> {
    let cli = Command::parse();
    let state = State::new().await?;

    notmad::Mad::builder()
        .add(state.drop_queue.clone())
        .add(CommandHandler::new(cli, &state))
        .run()
        .await
        .map_err(unwrap_run_errors)?;

    Ok(())
}

fn unwrap_run_errors(error: MadError) -> anyhow::Error {
    match error {
        MadError::Inner(error) => error,
        MadError::RunError { run } => run,
        MadError::CloseError { close } => close,
        MadError::AggregateError(aggregate_error) => anyhow::anyhow!("{}", aggregate_error),
        _ => todo!("error is not implemented, and not intended"),
    }
}

struct CommandHandler {
    state: State,
    cli: Command,
}

impl CommandHandler {
    fn new(cli: Command, state: &State) -> Self {
        Self {
            state: state.clone(),
            cli,
        }
    }

    async fn handle(&self) -> anyhow::Result<()> {
        let state = &self.state;
        let cli = &self.cli;

        match cli
            .command
            .as_ref()
            .expect("commands are required should've been caught by clap")
        {
            Commands::Init(init_command) => init_command.execute(state).await,
            Commands::Components(components_command) => components_command.execute(state).await,
            Commands::Admin(cmd) => cmd.execute(state).await,
            Commands::Run(cmd) => cmd.execute(state).await,
            Commands::Template(cmd) => cmd.execute(state).await,
            Commands::Publish(cmd) => cmd.execute(state).await,
            Commands::Global(cmd) => cmd.execute(state).await,
            Commands::Shell(cmd) => cmd.execute(state).await,
            Commands::Tmp(cmd) => cmd.execute(state).await,
            Commands::Project(cmd) => cmd.execute(state).await,
        }
    }
}

#[async_trait::async_trait]
impl Component for CommandHandler {
    fn name(&self) -> Option<String> {
        Some("non/command".into())
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        tokio::select! {
            _ = cancellation_token.cancelled() => {},
            res = self.handle() => {
                res?;
            }
        }

        Ok(())
    }
}
