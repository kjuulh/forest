use admin::AdminCommand;
use auth::AuthCommand;
use clap::{Parser, Subcommand};
use components::ComponentsCommand;
use global::GlobalCommand;
use init::InitCommand;
use notmad::{Component, ComponentInfo, MadError};
use run::RunCommand;
use shell::ShellCommand;
use template::TemplateCommand;
use tmp::TmpCommand;
use tokio_util::sync::CancellationToken;

use crate::{
    cli::{
        destination::DestinationCommand, notifications::NotificationsCommand,
        organisation::OrganisationCommand, project::ProjectCommand, release::ReleaseCommand,
    },
    state::{Config, State},
};

mod admin;
mod auth;
mod components;
mod destination;
mod global;
mod init;
mod notifications;
mod organisation;
mod project;
mod release;
mod run;
mod shell;
mod template;
mod tmp;

pub(crate) mod output;

#[derive(Parser)]
#[command(author, version, about, long_about, subcommand_required = true)]
struct Command {
    #[command(subcommand)]
    command: Option<Commands>,

    #[command(flatten)]
    config: Config,
}

#[derive(Subcommand)]
enum Commands {
    Init(InitCommand),
    Components(ComponentsCommand),
    Admin(AdminCommand),
    /// Authenticate and manage credentials
    Auth(AuthCommand),
    Run(RunCommand),
    Template(TemplateCommand),
    Global(GlobalCommand),
    Shell(ShellCommand),
    Tmp(TmpCommand),
    Project(ProjectCommand),
    Destination(DestinationCommand),
    Release(ReleaseCommand),
    /// Manage organisations
    Organisation(OrganisationCommand),
    /// Manage and listen for notifications
    Notifications(NotificationsCommand),
}

pub async fn execute() -> anyhow::Result<()> {
    let cli = Command::parse();
    let state = State::new(cli.config.clone()).await?;

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
        MadError::AggregateError(aggregate_error) => {
            let errors = aggregate_error
                .take_errors()
                .into_iter()
                .map(unwrap_run_errors)
                .collect::<Vec<_>>();

            let mut combined = Vec::new();

            for error in errors {
                combined.push(format!("{:?}", error));
            }

            anyhow::anyhow!(
                "{}",
                combined
                    .into_iter()
                    .filter(|i| !i.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            )
        }
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
            Commands::Auth(cmd) => cmd.execute(state).await,
            Commands::Run(cmd) => cmd.execute(state).await,
            Commands::Template(cmd) => cmd.execute(state).await,
            Commands::Global(cmd) => cmd.execute(state).await,
            Commands::Shell(cmd) => cmd.execute(state).await,
            Commands::Tmp(cmd) => cmd.execute(state).await,
            Commands::Project(cmd) => cmd.execute(state).await,
            Commands::Destination(cmd) => cmd.execute(state).await,
            Commands::Release(cmd) => cmd.execute(state).await,
            Commands::Organisation(cmd) => cmd.execute(state).await,
            Commands::Notifications(cmd) => cmd.execute(state).await,
        }
    }
}

impl Component for CommandHandler {
    fn info(&self) -> ComponentInfo {
        "forest/command".into()
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
