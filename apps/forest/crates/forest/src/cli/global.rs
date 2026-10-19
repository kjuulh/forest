use anyhow::Context;
use clap::{Args, Parser, Subcommand};

use crate::global::service::{GlobalService, SyncOutcome, ToolSource, ToolStatus, WarmEvent};
use crate::global::shim::QualifiedRef;
use crate::global::warm;
use crate::state::State;

mod global_init;
mod global_set;

#[derive(Parser)]
pub struct GlobalCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(Subcommand)]
#[clap(subcommand_required = true)]
enum Commands {
    /// Scaffold a new project at a directory (filesystem only — no server call).
    /// Renamed from `init` (kept as a hidden alias).
    #[command(alias = "init")]
    Scaffold(global_init::GlobalInitCommand),
    /// Set a user-config kv pair (forest global set <key> <value>).
    Set(global_set::GlobalSetCommand),
    /// Add a per-tool dependency: `<org>/<name>[@<version>]`.
    Add(AddCommand),
    /// Remove a per-tool dependency and its shim.
    Remove(RemoveCommand),
    /// List installed global tools.
    List(ListCommand),
    /// Run a global tool by name (shim entry point).
    Run(RunCommand),
    /// Print the absolute path of a resolved tool (cold-fetches if missing).
    Which(WhichCommand),
    /// Re-verify every cached binary; delete mismatches.
    Verify(VerifyCommand),
    /// Pre-download the binaries for global tools that aren't cached yet, so
    /// the first real invocation is instant. Designed to be called from a
    /// shell rc file as `forest global warm --background --quiet`: it detaches,
    /// prints nothing, and is throttled so repeated shell starts are free.
    Warm(WarmCommand),
    /// Repair shims if they drift from forest.cue (idempotent). Normally
    /// automatic — add/remove/ban/unban reconcile for you, so you only need
    /// this after hand-editing forest.cue or `add --no-sync`.
    Sync(SyncCommand),
    /// Re-resolve pins + catalogue subscriptions; bump to latest. Also runs
    /// automatically in the background (~once a day) when you invoke a global
    /// tool — set FOREST_NO_AUTO_UPDATE=1 to opt out.
    Update(UpdateCommand),
    /// Ban a tool from a catalogue subscription.
    Ban(BanCommand),
    /// Unban a tool from a catalogue subscription.
    Unban(UnbanCommand),
    /// Pin a tool's version inside a catalogue subscription.
    Pin(PinCommand),
    /// Unpin a tool inside a catalogue subscription.
    Unpin(UnpinCommand),
}

impl GlobalCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Scaffold(cmd) => cmd.execute(state).await,
            Commands::Set(cmd) => cmd.execute(state).await,
            Commands::Add(cmd) => cmd.execute(state).await,
            Commands::Remove(cmd) => cmd.execute(state).await,
            Commands::List(cmd) => cmd.execute(state).await,
            Commands::Run(cmd) => cmd.execute(state).await,
            Commands::Which(cmd) => cmd.execute(state).await,
            Commands::Verify(cmd) => cmd.execute(state).await,
            Commands::Warm(cmd) => cmd.execute(state).await,
            Commands::Sync(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
            Commands::Ban(cmd) => cmd.execute(state).await,
            Commands::Unban(cmd) => cmd.execute(state).await,
            Commands::Pin(cmd) => cmd.execute(state).await,
            Commands::Unpin(cmd) => cmd.execute(state).await,
        }
    }

    pub fn is_silent(&self) -> bool {
        match &self.commands {
            Commands::Warm(cmd) => cmd.background || cmd.quiet,
            _ => false,
        }
    }
}

#[derive(Args)]
pub struct WarmCommand {
    /// Only warm these tools — the name you type (a shim name) or a qualified
    /// `<org>/<name>`. Repeatable. Default: every installed tool.
    tools: Vec<String>,

    /// Return immediately, warming in a detached child. Nothing is printed and
    /// the child survives the shell that started it — the mode to use from a
    /// shell rc file.
    #[arg(long)]
    background: bool,

    /// Print nothing at all. Implied by `--background` for the child.
    #[arg(long, short)]
    quiet: bool,

    /// Ignore the throttle and warm now. Only meaningful with `--background`
    /// (a foreground warm is an explicit request and is never throttled).
    #[arg(long)]
    force: bool,
}

impl WarmCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;

        if self.background {
            if warm::disabled() {
                return Ok(());
            }
            if self.force {
                warm::spawn_detached();
            } else {
                warm::maybe_spawn(&svc.paths);
            }
            return Ok(());
        }

        if self.quiet {
            crate::ui::set_quiet(true);
        }

        let Some(_lock) = warm::WarmLock::acquire(&svc.paths) else {
            if !self.quiet {
                eprintln!("a warm is already running; nothing to do");
            }
            return Ok(());
        };

        let quiet = self.quiet;
        let outcome = svc
            .warm_tools(&self.tools, |ev| {
                if quiet {
                    return;
                }
                match ev {
                    WarmEvent::AlreadyWarm(t) => eprintln!("  = {} (cached)", t.shim_name),
                    WarmEvent::Fetching(_) => {}
                    WarmEvent::Fetched(t) => {
                        eprintln!("  + {}@{}", t.shim_name, t.version)
                    }
                    WarmEvent::Failed(t, e) => {
                        eprintln!("  ! {}: {e:#}", t.shim_name)
                    }
                    WarmEvent::CapturedShell(t, shells) => {
                        eprintln!(
                            "  ↻ {}: shell integration ({})",
                            t.shim_name,
                            shells.join(", ")
                        )
                    }
                    WarmEvent::Unknown(name) => {
                        eprintln!("  ? {name}: not an installed global tool")
                    }
                }
            })
            .await?;

        if !quiet {
            eprintln!(
                "warm: {} fetched, {} already cached, {} failed, {} shell integration(s) captured",
                outcome.fetched.len(),
                outcome.already_warm,
                outcome.failed.len(),
                outcome.shell_snippets,
            );
        }
        Ok(())
    }
}

#[derive(Args)]
pub struct SyncCommand {}

impl SyncCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;
        let out = svc.sync_shims().await?;
        eprintln!(
            "sync: {} shim(s) created, {} deleted",
            out.created.len(),
            out.deleted.len()
        );
        for s in &out.created {
            eprintln!("  + {s}");
        }
        for s in &out.deleted {
            eprintln!("  − {s}");
        }
        Ok(())
    }
}

#[derive(Args)]
pub struct UpdateCommand {
    /// Internal: invoked by the throttled background auto-update. Runs the
    /// update silently and swallows errors (the shell stdio is /dev/null
    /// anyway) so a flaky network never leaves stray output or a non-zero
    /// exit lying around. Not meant to be typed by hand.
    #[arg(long = "background", hide = true)]
    background: bool,
}

impl UpdateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;

        if self.background {
            let _ = svc.update_all().await;
            return Ok(());
        }

        let out = svc.update_all().await?;
        if out.bumps.is_empty() {
            eprintln!("no per-tool version bumps");
        } else {
            for b in &out.bumps {
                eprintln!("  {} : {} → {}", b.qualified, b.from, b.to);
            }
        }
        if out.held > 0 {
            eprintln!("held {} pinned tool(s) at their pinned version", out.held);
        }
        eprintln!(
            "sync: {} shim(s) created, {} deleted",
            out.sync.created.len(),
            out.sync.deleted.len()
        );
        Ok(())
    }
}

#[derive(Args)]
pub struct BanCommand {
    /// Organisation whose catalogue you've subscribed to.
    organisation: String,
    /// Upstream tool name to ban.
    tool: String,
}

impl BanCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;
        svc.ban_tool(&self.organisation, &self.tool).await?;
        eprintln!("banned {} from {} catalogue", self.tool, self.organisation);
        Ok(())
    }
}

#[derive(Args)]
pub struct UnbanCommand {
    organisation: String,
    tool: String,
}

impl UnbanCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;
        svc.unban_tool(&self.organisation, &self.tool).await?;
        eprintln!(
            "unbanned {} from {} catalogue",
            self.tool, self.organisation
        );
        match svc.sync_shims().await {
            Ok(out) => {
                for line in format_reconcile_lines(&out, "after unban") {
                    eprintln!("{line}");
                }
            }
            Err(e) => eprintln!(
                "warning: could not recreate shim now: {e:#}; run `forest global sync` to retry"
            ),
        }
        Ok(())
    }
}

#[derive(Args)]
pub struct PinCommand {
    /// `<org>/<tool>` — tool inside an existing org catalogue subscription.
    target: String,
    /// Version to pin.
    version: String,
}

impl PinCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;
        let (org, tool) = parse_org_tool(&self.target)?;
        svc.pin_catalogue_tool(&org, &tool, &self.version).await?;
        eprintln!("pinned {tool} to {} in {org}", self.version);
        Ok(())
    }
}

#[derive(Args)]
pub struct UnpinCommand {
    /// `<org>/<tool>`.
    target: String,
}

impl UnpinCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;
        let (org, tool) = parse_org_tool(&self.target)?;
        svc.unpin_catalogue_tool(&org, &tool).await?;
        eprintln!("unpinned {tool} in {org}");
        Ok(())
    }
}

fn parse_org_tool(raw: &str) -> anyhow::Result<(String, String)> {
    let (org, tool) = raw
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("expected `<org>/<tool>`, got {raw:?}"))?;
    if org.is_empty() || tool.is_empty() {
        anyhow::bail!("malformed reference: {raw:?}");
    }
    Ok((org.to_string(), tool.to_string()))
}

#[derive(Args)]
pub struct AddCommand {
    /// `<org>/<name>[@<ver>]` for per-tool, or bare `<org>` to subscribe to
    /// the org's whole tool catalogue.
    component: String,

    /// Override the shim name on disk (per-tool only).
    #[arg(long = "as")]
    as_shim: Option<String>,

    /// Ban a tool from a catalogue subscription. Repeatable.
    #[arg(long = "ban")]
    ban: Vec<String>,

    /// Pin a specific tool's version inside a catalogue subscription.
    /// Format: `name=version`. Repeatable.
    #[arg(long = "pin")]
    pin: Vec<String>,

    /// Alias a catalogue tool's shim name. Format: `upstream=local`.
    /// Repeatable.
    #[arg(long = "alias")]
    alias: Vec<String>,

    /// Skip the implicit `forest global sync` step after writing the
    /// dependency. Useful in scripts / CI that don't want extra network
    /// calls during `add`.
    #[arg(long = "no-sync")]
    no_sync: bool,
}

impl AddCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;

        if !self.component.contains('/') && !self.component.contains('@') {
            let pins = parse_kv_list(&self.pin, "--pin")?;
            let aliases = parse_kv_list(&self.alias, "--alias")?;
            let outcome = svc
                .subscribe_to_org(&self.component, &self.ban, &pins, &aliases)
                .await?;
            eprintln!(
                "subscribed to org catalogue '{}' ({} tools)",
                outcome.organisation,
                outcome.emitted.len()
            );
            for e in &outcome.emitted {
                eprintln!(
                    "  + {}  ({}@{})",
                    e.shim_name, e.qualified, e.resolved_version
                );
            }
            for b in &outcome.banned_seen {
                eprintln!("  − {}  BANNED", b);
            }
            for s in &outcome.shadowed {
                eprintln!("  · {}  shadowed by [dependencies]", s);
            }
            self.run_post_add_sync(&svc).await;
            return Ok(());
        }

        let (org, name, version) = parse_component_ref(&self.component)?;
        let outcome = svc
            .add_dependency(&org, &name, version.as_deref(), self.as_shim.as_deref())
            .await?;
        eprintln!(
            "added {}/{}@{} (shape={:?})",
            org, name, outcome.resolved_version, outcome.shape
        );
        if let Some(shim) = outcome.shim_name {
            eprintln!("shim created: {}", svc.shim_path(&shim).display());
        } else {
            eprintln!("(no tool facet — no shim created)");
        }
        self.run_post_add_sync(&svc).await;
        Ok(())
    }

    async fn run_post_add_sync(&self, svc: &GlobalService) {
        if self.no_sync {
            return;
        }
        match svc.sync_shims().await {
            Ok(out) => {
                for line in format_reconcile_lines(&out, "after add") {
                    eprintln!("{line}");
                }
            }
            Err(e) => {
                eprintln!(
                    "warning: post-add sync failed: {e:#}; run `forest global sync` to retry"
                );
            }
        }
    }
}

fn format_reconcile_lines(out: &SyncOutcome, context: &str) -> Vec<String> {
    if out.created.is_empty() && out.deleted.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::with_capacity(1 + out.created.len() + out.deleted.len());
    lines.push(format!(
        "sync ({context}): {} shim(s) created, {} deleted",
        out.created.len(),
        out.deleted.len()
    ));
    for s in &out.created {
        lines.push(format!("  + {s}"));
    }
    for s in &out.deleted {
        lines.push(format!("  − {s}"));
    }
    lines
}

fn parse_kv_list(items: &[String], flag: &str) -> anyhow::Result<Vec<(String, String)>> {
    items
        .iter()
        .map(|s| {
            let (k, v) = s
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("{flag} expects `name=value`, got {s:?}"))?;
            Ok((k.to_string(), v.to_string()))
        })
        .collect()
}

#[derive(Args)]
pub struct RemoveCommand {
    /// `<org>/<name>`.
    component: String,
}

impl RemoveCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;
        let (org, name, _) = parse_component_ref(&self.component)?;
        svc.remove_dependency(&org, &name).await?;
        eprintln!("removed {org}/{name}");
        Ok(())
    }
}

#[derive(Args)]
pub struct ListCommand {}

#[derive(serde::Serialize, tabled::Tabled)]
struct ListedToolRow {
    #[tabled(rename = "NAME")]
    name: String,
    #[tabled(rename = "ORG/NAME")]
    qualified: String,
    #[tabled(rename = "VERSION")]
    version: String,
    #[tabled(rename = "STATUS")]
    status: String,
    #[tabled(rename = "SOURCE")]
    source: String,
}

impl ListCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;
        let items = svc.list().await?;
        if items.is_empty() {
            use crate::cli::output::OutputFormat;
            match state.config.format {
                OutputFormat::Pretty | OutputFormat::Text => {
                    println!("(no global tools installed)");
                }
                OutputFormat::Name => {}
                OutputFormat::Json => println!("[]"),
            }
            return Ok(());
        }
        let rows: Vec<ListedToolRow> = items
            .into_iter()
            .map(|t| ListedToolRow {
                name: t.shim_name,
                qualified: format!("{}/{}", t.organisation, t.name),
                version: t.version,
                status: match t.status {
                    ToolStatus::Cached => "cached".to_string(),
                    ToolStatus::Missing => "missing".to_string(),
                },
                source: match t.source {
                    ToolSource::Pin => "[pinned]".to_string(),
                    ToolSource::Latest => "[latest]".to_string(),
                    ToolSource::Catalog { org } => format!("[catalog:{org}]"),
                    ToolSource::CatalogBanned { org } => format!("[catalog:{org} banned]"),
                    ToolSource::CatalogShadowed { org } => {
                        format!("[catalog:{org} shadowed by dependency]")
                    }
                },
            })
            .collect();
        print!(
            "{}",
            crate::cli::output::render(&state.config.format, &rows)
        );
        Ok(())
    }
}

#[derive(Args)]
pub struct RunCommand {
    /// Tool reference: `<bare-name>`, `<org>/<name>`, or `<org>/<name>@<ver>`.
    tool: String,

    /// Name to hand the binary as `argv[0]`. Shims pass the name they were
    /// invoked as, so an aliased tool sees the alias rather than the upstream
    /// component name. Defaults to the component name.
    #[arg(long = "as", value_name = "NAME")]
    invoked_as: Option<String>,

    /// Don't lazily download the tool: if its binary isn't cached yet, start a
    /// background warm and exit 75 (`EX_TEMPFAIL`) without running anything.
    ///
    /// This is what makes shell startup non-blocking (DATA-588). A rc file that
    /// does `eval "$(gitnow init zsh)"` only wants an init script; on a cold
    /// cache the honest answer is "not yet" rather than a multi-MB download
    /// with the prompt held hostage behind it. Also settable as
    /// `FOREST_GLOBAL_NO_FETCH=1`, which is how the `forest-init` shell helper
    /// applies it to tools it invokes by their own name (through the shim,
    /// where there is no forest command line to pass a flag on).
    #[arg(long = "no-fetch")]
    no_fetch: bool,

    /// Trailing args are forwarded to the underlying binary.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

impl RunCommand {
    fn no_fetch(&self) -> bool {
        self.no_fetch || std::env::var_os(warm::NO_FETCH_ENV).is_some()
    }

    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;
        let no_fetch = self.no_fetch();

        crate::global::autoupdate::maybe_spawn(&svc.paths);

        let resolved = resolve_tool_ref(&svc, &self.tool).await;
        let (qref, version) = match resolved {
            Ok(ResolvedRef::Qualified { qref, version }) => (qref, version),
            Err(e) if no_fetch => {
                tracing::debug!(tool = %self.tool, "no-fetch: cannot resolve: {e:#}");
                skip_uncached(&svc);
            }
            Err(e) => return Err(e),
        };

        let path = if no_fetch {
            match svc.cached_path_if_present(&qref, &version).await? {
                Some(p) => p,
                None => skip_uncached(&svc),
            }
        } else {
            svc.resolve_to_cached_path(&qref, &version).await?
        };

        let component_env = svc
            .load_tool_include_env(&qref, &version)
            .await
            .unwrap_or_default();
        let local_env = svc
            .load_user_config()
            .await
            .unwrap_or_default()
            .dependencies
            .get(&format!("{}/{}", qref.organisation, qref.name))
            .map(|d| d.env.clone())
            .unwrap_or_default();
        let ambient: std::collections::BTreeSet<String> = std::env::vars_os()
            .filter_map(|(k, _)| k.into_string().ok())
            .collect();
        let injected = crate::global::env::resolve_injection(&component_env, &local_env, &ambient);

        let arg0 = self
            .invoked_as
            .as_deref()
            .filter(|n| forest_manifest::names::validate_tool_name(n).is_ok())
            .unwrap_or(&qref.name);

        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new(&path);
        cmd.arg0(arg0);
        cmd.args(&self.args);
        cmd.envs(&injected);
        let err = cmd.exec();
        anyhow::bail!("failed to exec {}: {err}", path.display());
    }
}

fn skip_uncached(svc: &GlobalService) -> ! {
    warm::maybe_spawn(&svc.paths);
    std::process::exit(warm::EXIT_NOT_CACHED)
}

#[derive(Args)]
pub struct WhichCommand {
    tool: String,
    /// Print only the bare cached artifact path (back-compat for scripts
    /// that grep this output). Without this flag, the output also
    /// includes the resolved `<org>/<name>@<version>` qualifier and the
    /// shim path. TASKS/031 item #11.
    #[arg(long = "script")]
    script: bool,
}

impl WhichCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;
        let (qref, version) = match resolve_tool_ref(&svc, &self.tool).await? {
            ResolvedRef::Qualified { qref, version } => (qref, version),
        };
        let p = svc.resolve_to_cached_path(&qref, &version).await?;

        if self.script {
            println!("{}", p.display());
        } else {
            println!(
                "{}/{}@{} → {}",
                qref.organisation,
                qref.name,
                version,
                p.display()
            );
            let shim_path = svc.shim_path(&self.tool);
            if shim_path.exists() {
                println!("  shim:     {}", shim_path.display());
            }
        }
        Ok(())
    }
}

#[derive(Args)]
pub struct VerifyCommand {}

impl VerifyCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;
        if let Err(e) = svc.migrate_binary_store().await {
            tracing::debug!("binary-store migration skipped: {e:#}");
        }
        let deleted = svc.cache.re_verify().await?;
        if deleted.is_empty() {
            eprintln!("cache verified, no mismatches");
        } else {
            eprintln!("deleted {} mismatched entries:", deleted.len());
            for p in deleted {
                eprintln!("  {}", p.display());
            }
        }
        Ok(())
    }
}

enum ResolvedRef {
    Qualified { qref: QualifiedRef, version: String },
}

async fn resolve_tool_ref(svc: &GlobalService, raw: &str) -> anyhow::Result<ResolvedRef> {
    if raw.contains('/') {
        let (org, name, ver) = parse_component_ref(raw)?;
        let version = match ver {
            Some(v) => v,
            None => resolve_version(svc, &org, &name).await?,
        };
        Ok(ResolvedRef::Qualified {
            qref: QualifiedRef::new(org, name),
            version,
        })
    } else {
        let qref = svc.resolve_bare_name(raw).await?;
        let version = resolve_version(svc, &qref.organisation, &qref.name).await?;
        Ok(ResolvedRef::Qualified { qref, version })
    }
}

async fn resolve_version(svc: &GlobalService, org: &str, name: &str) -> anyhow::Result<String> {
    let cfg = svc.load_user_config().await?;
    let key = format!("{org}/{name}");

    if let Some(dep) = cfg.dependencies.get(&key) {
        return Ok(dep.version.clone());
    }

    if let Some(cat) = cfg.org_catalog.get(org)
        && cat.enabled
    {
        let entries = svc
            .grpc
            .list_org_tools(org)
            .await
            .with_context(|| format!("looking up catalogue version for {key}"))?;
        for entry in entries {
            if entry.name == name {
                let tool_name = entry
                    .tool
                    .as_ref()
                    .map(|t| t.name.as_str())
                    .unwrap_or(&entry.name);
                if cat.banned.iter().any(|b| b == tool_name) {
                    anyhow::bail!("{key} is banned in catalogue subscription {org}");
                }
                let v = cat
                    .pins
                    .get(tool_name)
                    .cloned()
                    .unwrap_or(entry.latest_version);
                return Ok(v);
            }
        }
    }

    anyhow::bail!(
        "{key} is not pinned in forest.cue — specify @<version> or run \
         `forest global add {key}` first"
    )
}

fn parse_component_ref(s: &str) -> anyhow::Result<(String, String, Option<String>)> {
    let (head, version) = match s.split_once('@') {
        Some((h, v)) => (h, Some(v.to_string())),
        None => (s, None),
    };
    let (org, name) = head
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("expected <org>/<name>[@<ver>], got {s:?}"))?;
    if org.is_empty() || name.is_empty() {
        anyhow::bail!("malformed reference: {s:?}");
    }
    Ok((org.to_string(), name.to_string(), version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct AddHarness {
        #[command(flatten)]
        add: AddCommand,
    }

    #[test]
    fn no_sync_flag_defaults_to_false() {
        let h = AddHarness::try_parse_from(["forest-global-add", "cuteorg/rg"]).unwrap();
        assert!(!h.add.no_sync);
    }

    #[test]
    fn no_sync_flag_is_recognised() {
        let h =
            AddHarness::try_parse_from(["forest-global-add", "cuteorg/rg", "--no-sync"]).unwrap();
        assert!(h.add.no_sync);
    }

    #[test]
    fn no_sync_works_with_catalogue_form() {
        let h = AddHarness::try_parse_from([
            "forest-global-add",
            "cuteorg",
            "--ban",
            "foo",
            "--no-sync",
        ])
        .unwrap();
        assert!(h.add.no_sync);
        assert_eq!(h.add.component, "cuteorg");
        assert_eq!(h.add.ban, vec!["foo".to_string()]);
    }

    #[test]
    fn no_sync_is_a_flag_not_a_value_arg() {
        let res = AddHarness::try_parse_from(["forest-global-add", "cuteorg/rg", "--no-sync=true"]);
        assert!(res.is_err(), "expected clap error, got Ok");
    }

    #[test]
    fn format_reconcile_lines_handles_many_entries() {
        let out = SyncOutcome {
            created: vec!["a".into(), "b".into()],
            deleted: vec!["c".into(), "d".into(), "e".into()],
        };
        let lines = format_reconcile_lines(&out, "after add");
        assert_eq!(lines.len(), 1 + 2 + 3);
        assert_eq!(lines[0], "sync (after add): 2 shim(s) created, 3 deleted");
        assert_eq!(&lines[1..3], &["  + a".to_string(), "  + b".to_string()]);
        assert_eq!(
            &lines[3..6],
            &[
                "  − c".to_string(),
                "  − d".to_string(),
                "  − e".to_string()
            ]
        );
    }

    #[test]
    fn format_reconcile_lines_is_silent_when_no_changes() {
        let out = SyncOutcome {
            created: vec![],
            deleted: vec![],
        };
        assert!(format_reconcile_lines(&out, "after add").is_empty());
    }

    #[test]
    fn format_reconcile_lines_reports_created_and_deleted() {
        let out = SyncOutcome {
            created: vec!["rg".into()],
            deleted: vec!["old".into()],
        };
        let lines = format_reconcile_lines(&out, "after add");
        assert_eq!(
            lines,
            vec![
                "sync (after add): 1 shim(s) created, 1 deleted".to_string(),
                "  + rg".to_string(),
                "  − old".to_string(),
            ]
        );
    }

    #[derive(Parser)]
    struct WarmHarness {
        #[command(flatten)]
        warm: WarmCommand,
    }

    #[derive(Parser)]
    struct RunHarness {
        #[command(flatten)]
        run: RunCommand,
    }

    #[test]
    fn warm_defaults_to_foreground_noisy_and_all_tools() {
        let h = WarmHarness::try_parse_from(["forest-global-warm"]).unwrap();
        assert!(!h.warm.background);
        assert!(!h.warm.quiet);
        assert!(!h.warm.force);
        assert!(h.warm.tools.is_empty(), "no args means every tool");
    }

    #[test]
    fn warm_accepts_the_rc_file_invocation() {
        let h =
            WarmHarness::try_parse_from(["forest-global-warm", "--background", "--quiet"]).unwrap();
        assert!(h.warm.background);
        assert!(h.warm.quiet);
    }

    #[test]
    fn warm_accepts_a_tool_list_alongside_flags() {
        let h = WarmHarness::try_parse_from([
            "forest-global-warm",
            "--quiet",
            "gitnow",
            "understory/awslogin",
        ])
        .unwrap();
        assert!(h.warm.quiet);
        assert_eq!(h.warm.tools, vec!["gitnow", "understory/awslogin"]);
    }

    #[test]
    fn warm_short_quiet_flag_works() {
        let h = WarmHarness::try_parse_from(["forest-global-warm", "-q"]).unwrap();
        assert!(h.warm.quiet);
    }

    #[test]
    fn background_or_quiet_warm_suppresses_the_update_nag() {
        for args in [
            vec!["forest-global", "warm", "--background"],
            vec!["forest-global", "warm", "--quiet"],
            vec!["forest-global", "warm", "--background", "--quiet"],
        ] {
            let cmd = GlobalCommand::try_parse_from(args.clone()).unwrap();
            assert!(cmd.is_silent(), "{args:?} must be silent");
        }
    }

    #[test]
    fn ordinary_global_commands_still_nag() {
        for args in [
            vec!["forest-global", "warm"],
            vec!["forest-global", "list"],
            vec!["forest-global", "sync"],
        ] {
            let cmd = GlobalCommand::try_parse_from(args.clone()).unwrap();
            assert!(!cmd.is_silent(), "{args:?} should not be silent");
        }
    }

    #[test]
    fn run_defaults_to_fetching() {
        let h = RunHarness::try_parse_from(["forest-global-run", "cuteorg/rg"]).unwrap();
        assert!(!h.run.no_fetch);
    }

    #[test]
    fn run_accepts_no_fetch_before_the_argv_separator() {
        let h = RunHarness::try_parse_from([
            "forest-global-run",
            "cuteorg/rg",
            "--no-fetch",
            "--",
            "--version",
        ])
        .unwrap();
        assert!(h.run.no_fetch);
        assert_eq!(h.run.args, vec!["--version"]);
    }

    #[test]
    fn no_fetch_env_var_alone_enables_the_guard() {
        let h = RunHarness::try_parse_from(["forest-global-run", "cuteorg/rg"]).unwrap();
        unsafe { std::env::set_var(crate::global::warm::NO_FETCH_ENV, "1") };
        let guarded = h.run.no_fetch();
        unsafe { std::env::remove_var(crate::global::warm::NO_FETCH_ENV) };
        assert!(guarded);
    }

    #[test]
    fn format_reconcile_lines_uses_the_given_context_label() {
        let out = SyncOutcome {
            created: vec!["rg".into()],
            deleted: vec![],
        };
        let lines = format_reconcile_lines(&out, "after unban");
        assert_eq!(lines[0], "sync (after unban): 1 shim(s) created, 0 deleted");
    }
}
