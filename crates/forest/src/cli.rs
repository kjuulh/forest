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
        components::build::BuildCommand,
        components::generate::GenerateCommand,
        components::publish::PublishCommand,
        destination::DestinationCommand, environment::EnvironmentCommand,
        notifications::NotificationsCommand, organisation::OrganisationCommand,
        project::ProjectCommand, release::ReleaseCommand,
    },
    state::{Config, State},
};

mod add;
mod admin;
mod auth;
mod components;
mod destination;
mod docs;
mod environment;
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
mod update;
mod validate;

pub(crate) mod output;
pub(crate) mod prompts;

#[derive(Parser)]
#[command(
    author,
    version,
    about,
    long_about,
    subcommand_required = true,
    after_help = "Run 'forest docs' for comprehensive documentation, component authoring guides, and configuration reference."
)]
struct Command {
    #[command(subcommand)]
    command: Option<Commands>,

    #[command(flatten)]
    config: Config,
}

#[derive(Subcommand)]
enum Commands {
    /// Scaffold a new project or component
    Init(InitCommand),
    /// Add a component dependency to the project
    Add(add::AddCommand),

    // ── Component lifecycle (like cargo build/publish) ──
    /// Build the component binary for all configured platforms
    Build(BuildCommand),
    /// Generate type-safe code from CUE component spec (forest.component.cue)
    Generate(GenerateCommand),
    /// Publish component to the registry (binary + CUE spec + manifest)
    Publish(PublishCommand),
    /// Validate project config against component specs and check contract coverage
    Validate(validate::ValidateCommand),
    /// Update dependencies to the latest versions matching the spec
    Update(update::UpdateCommand),

    // ── Project commands ──
    /// Run a project or component command (e.g., forest run status)
    Run(RunCommand),
    /// Prepare, annotate, and execute releases
    Release(Box<ReleaseCommand>),

    // ── Resource management ──
    /// Manage projects
    Project(ProjectCommand),
    /// Manage deployment destinations
    Destination(DestinationCommand),
    /// Manage environments (dev, staging, prod)
    Environment(EnvironmentCommand),
    /// Manage organisations and members
    Organisation(OrganisationCommand),
    /// Manage and listen for notifications
    Notifications(NotificationsCommand),

    // ── Component registry ──
    /// Browse and manage components (list, init)
    Components(ComponentsCommand),

    /// Show comprehensive documentation and manpages
    Docs(docs::DocsCommand),

    // ── System (hidden from default help) ──
    /// Admin operations (server status, diagnostics)
    #[command(hide = true)]
    Admin(AdminCommand),
    /// Authenticate and manage credentials
    Auth(AuthCommand),
    /// Render templates
    #[command(hide = true)]
    Template(TemplateCommand),
    /// Manage global user configuration
    #[command(hide = true)]
    Global(GlobalCommand),
    /// Open an interactive shell
    #[command(hide = true)]
    Shell(ShellCommand),
    /// Manage temporary directories
    #[command(hide = true)]
    Tmp(TmpCommand),
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
            Commands::Init(cmd) => cmd.execute(state).await,
            Commands::Add(cmd) => cmd.execute(state).await,
            Commands::Build(cmd) => cmd.execute(state).await,
            Commands::Generate(cmd) => cmd.execute(state).await,
            Commands::Publish(cmd) => cmd.execute(state).await,
            Commands::Validate(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
            Commands::Run(cmd) => cmd.execute(state).await,
            Commands::Release(cmd) => cmd.execute(state).await,
            Commands::Project(cmd) => cmd.execute(state).await,
            Commands::Destination(cmd) => cmd.execute(state).await,
            Commands::Environment(cmd) => cmd.execute(state).await,
            Commands::Organisation(cmd) => cmd.execute(state).await,
            Commands::Notifications(cmd) => cmd.execute(state).await,
            Commands::Components(cmd) => cmd.execute(state).await,
            Commands::Docs(cmd) => cmd.execute(state).await,
            Commands::Admin(cmd) => cmd.execute(state).await,
            Commands::Auth(cmd) => cmd.execute(state).await,
            Commands::Template(cmd) => cmd.execute(state).await,
            Commands::Global(cmd) => cmd.execute(state).await,
            Commands::Shell(cmd) => cmd.execute(state).await,
            Commands::Tmp(cmd) => cmd.execute(state).await,
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
