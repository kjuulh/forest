//! `forest context …` — kubectl-style profile switcher.
//!
//! See TASKS/019-context.md. Each context is a `(server URL, auth state)`
//! bundle. One context is active at a time; the active one is selected by
//! per-invocation `--context`, `FOREST_CONTEXT`, or the registry's `active`
//! field.

use clap::{Args, Parser, Subcommand};

use crate::contexts::ContextStore;
use crate::state::State;
use crate::user_state::UserStateLoaderState;

#[derive(Parser)]
pub struct ContextCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(Subcommand)]
#[clap(subcommand_required = true)]
enum Commands {
    /// List all known contexts (the active one is marked with `*`).
    List(ListCommand),
    /// Print the active context's name + server URL.
    Active(ActiveCommand),
    /// Switch the active context.
    Use(UseCommand),
    /// Create a new context.
    Create(CreateCommand),
    /// Delete a context (and its auth state).
    Delete(DeleteCommand),
    /// Rename a context.
    Rename(RenameCommand),
    /// Update a context's server URL.
    SetServer(SetServerCommand),
    /// Update (or clear with `--clear`) a context's web URL — where
    /// `forest auth login --web` opens the browser. Falls back to the
    /// "strip leading `api.`" convention when unset.
    SetWebUrl(SetWebUrlCommand),
    /// Install-time provisioning. Idempotent — re-running with the
    /// same name updates the server. Used by `install.sh` to seed a
    /// default context from a `FOREST_PROFILE` env var. If no context
    /// exists yet, the provisioned one becomes active; otherwise it
    /// is added without changing the active context.
    Provision(ProvisionCommand),
}

impl ContextCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Active(cmd) => cmd.execute(state).await,
            Commands::Use(cmd) => cmd.execute(state).await,
            Commands::Create(cmd) => cmd.execute(state).await,
            Commands::Delete(cmd) => cmd.execute(state).await,
            Commands::Rename(cmd) => cmd.execute(state).await,
            Commands::SetServer(cmd) => cmd.execute(state).await,
            Commands::SetWebUrl(cmd) => cmd.execute(state).await,
            Commands::Provision(cmd) => cmd.execute(state).await,
        }
    }
}

// --- list ----------------------------------------------------------------

#[derive(Args)]
pub struct ListCommand {}

#[derive(serde::Serialize, tabled::Tabled)]
struct ContextRow {
    #[tabled(rename = "NAME")]
    name: String,
    #[tabled(rename = "SERVER")]
    server: String,
    #[tabled(rename = "USER")]
    user: String,
    #[tabled(rename = "ACTIVE")]
    active: String,
}

impl ListCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let store = ContextStore::from_env()?;
        let file = store.list()?;
        if file.contexts.is_empty() {
            use crate::cli::output::OutputFormat;
            match state.config.format {
                OutputFormat::Pretty | OutputFormat::Text => {
                    println!("(no contexts — run `forest context create <name> --server <url>`)");
                }
                OutputFormat::Name => {}
                OutputFormat::Json => println!("[]"),
            }
            return Ok(());
        }
        let rows: Vec<ContextRow> = file
            .contexts
            .iter()
            .map(|c| {
                let user_state_path = store.context_dir(&c.name).join("user-state.json");
                let user = match std::fs::read(&user_state_path) {
                    Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)
                        .ok()
                        .and_then(|v| {
                            v.get("username")
                                .and_then(|u| u.as_str())
                                .map(str::to_string)
                        })
                        .unwrap_or_else(|| "(not logged in)".to_string()),
                    Err(_) => "(not logged in)".to_string(),
                };
                ContextRow {
                    name: c.name.clone(),
                    server: c.server.clone(),
                    user,
                    active: if c.name == file.active { "*".to_string() } else { String::new() },
                }
            })
            .collect();
        print!("{}", crate::cli::output::render(&state.config.format, &rows));
        let _ = state.user_state();
        Ok(())
    }
}

// --- active --------------------------------------------------------------

#[derive(Args)]
pub struct ActiveCommand {}

impl ActiveCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        let store = ContextStore::from_env()?;
        let entry = store.active()?;
        println!("{}    {}", entry.name, entry.server);
        Ok(())
    }
}

// --- use -----------------------------------------------------------------

#[derive(Args)]
pub struct UseCommand {
    name: String,
}

impl UseCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        let store = ContextStore::from_env()?;
        store.use_context(&self.name)?;
        let entry = store.active()?;
        eprintln!("switched to context '{}' ({})", entry.name, entry.server);
        Ok(())
    }
}

// --- create --------------------------------------------------------------

#[derive(Args)]
pub struct CreateCommand {
    name: String,
    #[arg(long)]
    server: String,
    /// Also switch the active context to the new one.
    #[arg(long = "use")]
    switch_to: bool,
}

impl CreateCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        let store = ContextStore::from_env()?;
        let entry = store.create(&self.name, &self.server, self.switch_to)?;
        eprintln!(
            "created context '{}' (server={}){}",
            entry.name,
            entry.server,
            if self.switch_to { " — active" } else { "" }
        );
        Ok(())
    }
}

// --- delete --------------------------------------------------------------

#[derive(Args)]
pub struct DeleteCommand {
    name: String,
    /// Allow deleting the currently-active context.
    #[arg(long)]
    force: bool,
    /// Keep the on-disk directory (only the registry entry is removed).
    #[arg(long)]
    keep_data: bool,
}

impl DeleteCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        let store = ContextStore::from_env()?;
        store.delete(&self.name, self.force, self.keep_data)?;
        eprintln!("deleted context '{}'", self.name);
        Ok(())
    }
}

// --- rename --------------------------------------------------------------

#[derive(Args)]
pub struct RenameCommand {
    old: String,
    new: String,
}

impl RenameCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        let store = ContextStore::from_env()?;
        store.rename(&self.old, &self.new)?;
        eprintln!("renamed '{}' → '{}'", self.old, self.new);
        Ok(())
    }
}

// --- set-server ----------------------------------------------------------

#[derive(Args)]
pub struct SetServerCommand {
    name: String,
    server: String,
}

impl SetServerCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        let store = ContextStore::from_env()?;
        store.set_server(&self.name, &self.server)?;
        eprintln!("set {} server to {}", self.name, self.server);
        Ok(())
    }
}

// --- set-web-url ---------------------------------------------------------

#[derive(Args)]
pub struct SetWebUrlCommand {
    name: String,
    /// New web URL. Mutually exclusive with `--clear`.
    web_url: Option<String>,
    /// Clear the stored web URL (fall back to convention / env var).
    #[arg(long, conflicts_with = "web_url")]
    clear: bool,
}

impl SetWebUrlCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        let store = ContextStore::from_env()?;
        let new = if self.clear {
            None
        } else {
            self.web_url.as_deref().ok_or_else(|| {
                anyhow::anyhow!("pass a URL or `--clear` to remove the stored web URL")
            })?;
            self.web_url.as_deref()
        };
        store.set_web_url(&self.name, new)?;
        match new {
            Some(url) => eprintln!("set {} web URL to {url}", self.name),
            None => eprintln!("cleared web URL for {}", self.name),
        }
        Ok(())
    }
}

// --- provision -----------------------------------------------------------

#[derive(Args)]
pub struct ProvisionCommand {
    /// Context name (e.g. `understory-prod`). Must match the same
    /// validation rules as other context names.
    #[arg(long)]
    name: String,
    /// Forest server URL (e.g. `https://api.forest.understory.sh`).
    #[arg(long)]
    server: String,
    /// Optional web UI URL. When omitted, the CLI falls back to a
    /// "strip leading `api.`" convention. Pass-through from
    /// FOREST_PROFILE `web=` key.
    #[arg(long = "web-url")]
    web_url: Option<String>,
}

impl ProvisionCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        let store = ContextStore::from_env()?;
        let was_first = !store.contexts_file().exists();
        let entry = store.provision(&self.name, &self.server)?;
        if let Some(web) = self.web_url.as_deref() {
            store.set_web_url(&self.name, Some(web))?;
        }
        if was_first {
            eprintln!(
                "provisioned '{}' ({}) and set as active context",
                entry.name, entry.server
            );
        } else {
            eprintln!(
                "provisioned '{}' ({}) (active context unchanged — use `forest context use {}` to switch)",
                entry.name, entry.server, entry.name
            );
        }
        Ok(())
    }
}
