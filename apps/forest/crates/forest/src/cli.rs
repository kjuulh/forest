use admin::AdminCommand;
use auth::AuthCommand;
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
mod self_cmd;
mod shell;
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
    /// Manage named server+auth profiles (like kubectl context)
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

    /// Update the forest CLI itself (`self update` / `self check`)
    #[command(name = "self")]
    Self_(self_cmd::SelfCommand),
}

pub async fn execute() -> anyhow::Result<()> {
    let cli = Command::parse();

    // Resolve the active context once, up front. Two outcomes ride
    // on this:
    //   1. CUE_REGISTRY gets derived from the context's server if
    //      it's not already set in the parent shell, so users don't
    //      have to remember to export it after switching contexts.
    //   2. We print a kubectl-style banner so the active context is
    //      always visible at command start.
    //
    // The resolve is best-effort — a broken or missing context store
    // shouldn't prevent the command from running (in particular
    // `forest context provision …` itself runs before any context
    // exists). On failure we just skip both banner and overlay.
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

/// Resolve the active context and set `CUE_REGISTRY` in the process
/// environment if it's not already there. Pure side-effect; silent
/// on failure.
///
/// Uses `try_resolve` (not `resolve`) so we never trigger the
/// first-run bootstrap as a side effect — that would race with
/// `forest context provision`, which expects the contexts file to
/// be absent.
fn apply_context_env_overlay(cli: &Command) {
    // Skip when the user has already set CUE_REGISTRY explicitly —
    // they're overriding the derived value on purpose.
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

/// Print a kubectl-style one-line banner identifying the active
/// context. Skips for noisy / non-interactive cases:
///   - `forest context …` (the user is already looking at contexts;
///     the banner would compete with the command's own output).
///   - `forest self …` (the binary may be mid-replacement).
///   - stderr is not a TTY (piped output, redirected to a file).
///   - `NO_COLOR=1` *and* the command is one of the above — actually
///     NO_COLOR just suppresses colour, the banner still prints.
fn maybe_print_context_banner(cli: &Command) {
    use std::io::IsTerminal;
    let Some(command) = cli.command.as_ref() else {
        return;
    };
    if matches!(command, Commands::Context(_) | Commands::Self_(_)) {
        return;
    }
    if !std::io::stderr().is_terminal() {
        return;
    }
    let Ok(store) = crate::contexts::ContextStore::from_env() else {
        return;
    };
    // Same `try_resolve` rule as the env overlay: never bootstrap a
    // default context just to print a banner. If the file doesn't
    // exist (very first install before provision), silently skip.
    let Ok(Some(entry)) = store.try_resolve(cli.config.context.as_deref()) else {
        return;
    };

    if std::env::var_os("NO_COLOR").is_some() {
        eprintln!("◆ {}", entry.name);
    } else {
        // Dim diamond + cyan bold name. Bright enough to be obvious
        // without overwhelming the actual command output.
        eprintln!("\x1b[2m◆\x1b[0m \x1b[1;36m{}\x1b[0m", entry.name);
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

        // Skip the auto-nag for `forest self …` — the user is already
        // engaging with the update flow, no need to prompt them again,
        // and `self update` would print the nag while the binary is
        // mid-replacement.
        let print_nag = !matches!(command, Commands::Self_(_));

        let result = match command {
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
            Commands::Context(cmd) => cmd.execute(state).await,
            Commands::Tool(cmd) => cmd.execute(state).await,
            Commands::Template(cmd) => cmd.execute(state).await,
            Commands::Global(cmd) => cmd.execute(state).await,
            Commands::Shell(cmd) => cmd.execute(state).await,
            Commands::Tmp(cmd) => cmd.execute(state).await,
            Commands::Self_(cmd) => cmd.execute(state).await,
        };

        // Only nag on success — if the user's command already failed
        // we shouldn't add noise. The nag itself is best-effort and
        // silent on its own failures.
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
