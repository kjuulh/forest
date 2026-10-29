use admin::AdminCommand;
use auth::AuthCommand;
use bootstrap::BootstrapCommand;
use clap::{Parser, Subcommand};
use components::ComponentsCommand;
use context::ContextCommand;
use global::GlobalCommand;
use init::InitCommand;
use notmad::{Component, ComponentInfo, MadError};
use run::RunCommand;
use shell::ShellCommand;
use template::TemplateCommand;
use tmp::TmpCommand;
use tokio_util::sync::CancellationToken;
use tool::ToolCommand;

use crate::{
    cli::{
        components::generate::GenerateCommand, components::publish::PublishCommand,
        destination::DestinationCommand, environment::EnvironmentCommand,
        notifications::NotificationsCommand, organisation::OrganisationCommand,
        project::ProjectCommand, release::ReleaseCommand,
    },
    state::{Config, State},
};

mod add;
mod admin;
mod auth;
mod bootstrap;
mod components;
mod context;
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
mod run_output;
mod self_cmd;
mod shell;
mod state;
mod template;
mod tmp;
mod tool;
mod update;
mod validate;

pub(crate) mod output;
pub(crate) mod prompts;

#[derive(Parser)]
#[command(
    author,
    version,
    long_version = self_cmd::long_version(),
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

    /// Generate type-safe code from CUE component spec (forest.component.cue)
    Generate(GenerateCommand),
    /// Publish component to the registry (binary + CUE spec + manifest)
    Publish(PublishCommand),
    /// Validate project config against component specs and check contract coverage
    Validate(validate::ValidateCommand),
    /// Update dependencies to the latest versions matching the spec
    Update(update::UpdateCommand),

    /// Run a project or component command (e.g., forest run status)
    #[command(aliases = ["r"])]
    Run(RunCommand),
    /// Prepare, annotate, and execute releases
    Release(Box<ReleaseCommand>),

    /// Manage projects
    #[command(visible_aliases = ["projects", "proj"], aliases = ["p"])]
    Project(ProjectCommand),
    /// Manage deployment destinations
    #[command(visible_aliases = ["destinations", "dest"], aliases = ["d"])]
    Destination(DestinationCommand),
    /// Reach a project's terraform state outside a release (org Admin): credentials, pull, list, unlock
    State(state::StateCommand),
    /// Manage environments (dev, staging, prod)
    #[command(visible_aliases = ["environments", "env"], aliases = ["e"])]
    Environment(EnvironmentCommand),
    /// Manage organisations and members
    #[command(visible_aliases = ["organisations", "org", "orgs"], aliases = ["o"])]
    Organisation(OrganisationCommand),
    /// Manage and listen for notifications
    #[command(visible_aliases = ["notification", "notif"], aliases = ["n"])]
    Notifications(NotificationsCommand),

    /// Browse and manage components (list, init)
    #[command(visible_aliases = ["component", "comp"], aliases = ["c"])]
    Components(ComponentsCommand),

    /// Show comprehensive documentation and manpages
    Docs(docs::DocsCommand),

    /// Admin operations (server status, diagnostics)
    #[command(hide = true)]
    Admin(AdminCommand),
    /// Authenticate and manage credentials
    Auth(AuthCommand),
    /// Manage named server+auth profiles (like kubectl context)
    #[command(visible_aliases = ["contexts", "ctx"])]
    Context(ContextCommand),
    /// Helper commands for authoring tool manifests.
    Tool(ToolCommand),
    /// Render templates
    #[command(hide = true)]
    Template(TemplateCommand),
    /// Manage global user configuration
    #[command(hide = true)]
    Global(GlobalCommand),
    /// Print shell integration script — `eval "$(forest shell zsh)"` in your rc file.
    Shell(ShellCommand),
    /// Manage temporary directories
    #[command(hide = true)]
    Tmp(TmpCommand),

    /// Publish all in-repo workspace components in dependency order (dev helper)
    #[command(hide = true)]
    Bootstrap(BootstrapCommand),

    /// Update the forest CLI itself (`self update` / `self check`)
    #[command(name = "self")]
    Self_(self_cmd::SelfCommand),
}

pub async fn execute() -> anyhow::Result<()> {
    let cli = Command::parse();

    crate::ui::init_logging(cli.config.verbose);

    apply_context_env_overlay(&cli);
    maybe_print_context_banner(&cli);

    let state = State::new(cli.config.clone()).await?;

    notmad::Mad::builder()
        .add(state.drop_queue.clone())
        .add(CommandHandler::new(cli, &state))
        .run()
        .await
        .map_err(unwrap_run_errors)?;

    Ok(())
}

fn apply_context_env_overlay(cli: &Command) {
    if std::env::var_os("CUE_REGISTRY").is_some() {
        return;
    }
    let Ok(store) = crate::contexts::ContextStore::from_env() else {
        return;
    };
    let Ok(Some(entry)) = store.try_resolve(cli.config.context.as_deref()) else {
        return;
    };
    let Some(registry) = crate::contexts::derive_cue_registry(&entry.server) else {
        return;
    };
    // SAFETY: we're at the top of `execute()`, before any other
    // thread has been spawned (Mad / tokio runtime have not yet
    // touched env). `std::env::set_var` is marked unsafe in newer
    // Rust precisely because of cross-thread races; here there are
    // none.
    unsafe {
        std::env::set_var("CUE_REGISTRY", registry);
    }
}

fn maybe_print_context_banner(cli: &Command) {
    use std::io::IsTerminal;
    let Some(command) = cli.command.as_ref() else {
        return;
    };
    if !is_server_mutation(command) {
        return;
    }
    if !std::io::stderr().is_terminal() {
        return;
    }
    let Ok(store) = crate::contexts::ContextStore::from_env() else {
        return;
    };
    let Ok(Some(entry)) = store.try_resolve(cli.config.context.as_deref()) else {
        return;
    };
    if entry.name == "default" {
        return;
    }

    if std::env::var_os("NO_COLOR").is_some() {
        eprintln!("◆ {}", entry.name);
    } else {
        eprintln!("\x1b[2m◆\x1b[0m \x1b[1;36m{}\x1b[0m", entry.name);
    }
}

fn is_server_mutation(command: &Commands) -> bool {
    match command {
        Commands::Publish(_) | Commands::Release(_) | Commands::Bootstrap(_) => true,
        Commands::Auth(cmd) => cmd.is_mutation(),
        Commands::Project(cmd) => cmd.is_mutation(),
        Commands::Destination(cmd) => cmd.is_mutation(),
        Commands::State(cmd) => cmd.is_mutation(),
        Commands::Environment(cmd) => cmd.is_mutation(),
        Commands::Organisation(cmd) => cmd.is_mutation(),
        Commands::Notifications(cmd) => cmd.is_mutation(),
        Commands::Components(cmd) => cmd.is_mutation(),
        Commands::Init(_)
        | Commands::Add(_)
        | Commands::Generate(_)
        | Commands::Validate(_)
        | Commands::Update(_)
        | Commands::Run(_)
        | Commands::Docs(_)
        | Commands::Admin(_)
        | Commands::Context(_)
        | Commands::Tool(_)
        | Commands::Template(_)
        | Commands::Global(_)
        | Commands::Shell(_)
        | Commands::Tmp(_)
        | Commands::Self_(_) => false,
    }
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

        let command = cli
            .command
            .as_ref()
            .expect("commands are required should've been caught by clap");

        let print_nag = match command {
            Commands::Self_(_) => false,
            Commands::Global(cmd) => !cmd.is_silent(),
            _ => true,
        };

        let result = match command {
            Commands::Init(cmd) => cmd.execute(state).await,
            Commands::Add(cmd) => cmd.execute(state).await,
            Commands::Generate(cmd) => cmd.execute(state).await,
            Commands::Publish(cmd) => cmd.execute(state).await,
            Commands::Validate(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
            Commands::Run(cmd) => cmd.execute(state).await,
            Commands::Release(cmd) => cmd.execute(state).await,
            Commands::Project(cmd) => cmd.execute(state).await,
            Commands::Destination(cmd) => cmd.execute(state).await,
            Commands::State(cmd) => cmd.execute(state).await,
            Commands::Environment(cmd) => cmd.execute(state).await,
            Commands::Organisation(cmd) => cmd.execute(state).await,
            Commands::Notifications(cmd) => cmd.execute(state).await,
            Commands::Components(cmd) => cmd.execute(state).await,
            Commands::Docs(cmd) => cmd.execute(state).await,
            Commands::Admin(cmd) => cmd.execute(state).await,
            Commands::Auth(cmd) => cmd.execute(state).await,
            Commands::Context(cmd) => cmd.execute(state).await,
            Commands::Tool(cmd) => cmd.execute(state).await,
            Commands::Template(cmd) => cmd.execute(state).await,
            Commands::Global(cmd) => cmd.execute(state).await,
            Commands::Shell(cmd) => cmd.execute(state).await,
            Commands::Tmp(cmd) => cmd.execute(state).await,
            Commands::Bootstrap(cmd) => cmd.execute(state).await,
            Commands::Self_(cmd) => cmd.execute(state).await,
        };

        if print_nag && result.is_ok() {
            self_cmd::maybe_print_update_nag().await;
        }

        result
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

#[cfg(test)]
mod global_flag_tests {
    use super::*;

    #[test]
    fn forest_server_is_accepted_after_the_subcommand() {
        let cmd = Command::try_parse_from([
            "forest",
            "publish",
            "--dry-run",
            "--forest-server",
            "https://api.example",
        ])
        .unwrap();
        assert_eq!(
            cmd.config.forest_server.as_deref(),
            Some("https://api.example")
        );
    }

    #[test]
    fn context_is_accepted_after_the_subcommand() {
        let cmd = Command::try_parse_from(["forest", "publish", "--context", "work"]).unwrap();
        assert_eq!(cmd.config.context.as_deref(), Some("work"));
    }
}

#[cfg(test)]
mod release_parser_tests {
    use super::*;

    #[test]
    fn release_rejects_ref_instead_of_ignoring_it() {
        for subcommand in [None, Some("release")] {
            for slug in [None, Some("prepared-slug")] {
                for option in ["--ref", "-r"] {
                    let error = Command::try_parse_from(
                        ["forest", "release"]
                            .into_iter()
                            .chain(subcommand)
                            .chain(slug)
                            .chain([option, "main", "-e", "dev"]),
                    )
                    .err()
                    .expect("release must reject a ref argument");
                    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
                }
            }
        }
    }
}
