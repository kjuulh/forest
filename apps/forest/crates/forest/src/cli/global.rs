//! `forest global …` — user-global tool management. See TASKS/018-global-tools.md.

use anyhow::Context;
use clap::{Args, Parser, Subcommand};

use crate::global::service::{GlobalService, SyncOutcome, ToolSource, ToolStatus};
use crate::global::shim::QualifiedRef;
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
    /// Reconcile shims with forest.cue (idempotent).
    Sync(SyncCommand),
    /// Re-resolve pins + catalogue subscriptions; bump to latest.
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
            Commands::Sync(cmd) => cmd.execute(state).await,
            Commands::Update(cmd) => cmd.execute(state).await,
            Commands::Ban(cmd) => cmd.execute(state).await,
            Commands::Unban(cmd) => cmd.execute(state).await,
            Commands::Pin(cmd) => cmd.execute(state).await,
            Commands::Unpin(cmd) => cmd.execute(state).await,
        }
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
pub struct UpdateCommand {}

impl UpdateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;
        let out = svc.update_all().await?;
        if out.bumps.is_empty() {
            eprintln!("no per-tool version bumps");
        } else {
            for b in &out.bumps {
                eprintln!("  {} : {} → {}", b.qualified, b.from, b.to);
            }
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
            "unbanned {} (run `forest global sync` or `forest global update` to recreate the shim)",
            self.tool
        );
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

// --- add ------------------------------------------------------------------

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

        // Bare `<org>` → catalogue subscription mode.
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

        // Per-tool path.
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

    /// Reconcile shims with `forest.cue` after a successful add. The
    /// dependency has already been persisted, so a sync failure here is
    /// surfaced as a warning — the user can re-run `forest global sync`.
    async fn run_post_add_sync(&self, svc: &GlobalService) {
        if self.no_sync {
            return;
        }
        match svc.sync_shims().await {
            Ok(out) => {
                for line in format_post_add_sync(&out) {
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

/// Render the stderr lines for the post-add sync step. Returns an empty
/// vec when there is nothing to report (no shims created or deleted) so
/// callers can stay quiet in the common case.
fn format_post_add_sync(out: &SyncOutcome) -> Vec<String> {
    if out.created.is_empty() && out.deleted.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::with_capacity(1 + out.created.len() + out.deleted.len());
    lines.push(format!(
        "sync (after add): {} shim(s) created, {} deleted",
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
            let (k, v) = s.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("{flag} expects `name=value`, got {s:?}")
            })?;
            Ok((k.to_string(), v.to_string()))
        })
        .collect()
}

// --- remove ---------------------------------------------------------------

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

// --- list -----------------------------------------------------------------

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
            // Pretty / Text → friendly notice; Name / Json → empty output.
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
                    ToolSource::Pin => "[pin]".to_string(),
                    ToolSource::Catalog { org } => format!("[catalog:{org}]"),
                    ToolSource::CatalogBanned { org } => format!("[catalog:{org} banned]"),
                    ToolSource::CatalogShadowed { org } => {
                        format!("[catalog:{org} shadowed by pin]")
                    }
                },
            })
            .collect();
        print!("{}", crate::cli::output::render(&state.config.format, &rows));
        Ok(())
    }
}

// --- run ------------------------------------------------------------------

#[derive(Args)]
pub struct RunCommand {
    /// Tool reference: `<bare-name>`, `<org>/<name>`, or `<org>/<name>@<ver>`.
    tool: String,

    /// Trailing args are forwarded to the underlying binary.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

impl RunCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;

        let (qref, version) = match resolve_tool_ref(&svc, &self.tool).await? {
            ResolvedRef::Qualified { qref, version } => (qref, version),
        };

        let path = svc.resolve_to_cached_path(&qref, &version).await?;

        // Exec.
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&path).args(&self.args).exec();
        anyhow::bail!("failed to exec {}: {err}", path.display());
    }
}

// --- which ----------------------------------------------------------------

#[derive(Args)]
pub struct WhichCommand {
    tool: String,
}

impl WhichCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;
        let (qref, version) = match resolve_tool_ref(&svc, &self.tool).await? {
            ResolvedRef::Qualified { qref, version } => (qref, version),
        };
        let p = svc.resolve_to_cached_path(&qref, &version).await?;
        println!("{}", p.display());
        Ok(())
    }
}

// --- verify ---------------------------------------------------------------

#[derive(Args)]
pub struct VerifyCommand {}

impl VerifyCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let svc = GlobalService::from_state(state)?;
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

// --- helpers --------------------------------------------------------------

enum ResolvedRef {
    Qualified { qref: QualifiedRef, version: String },
}

async fn resolve_tool_ref(svc: &GlobalService, raw: &str) -> anyhow::Result<ResolvedRef> {
    // Cases:
    //   "<org>/<name>@<ver>"      — explicit, no lookup needed
    //   "<org>/<name>"            — version from forest.cue (pin OR catalogue)
    //   "<bare-name>"             — qualified via shim dir, then version from forest.cue
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

/// Find the version pin for `<org>/<name>` by looking in (in order):
///   1. `config.dependencies` (explicit per-tool pin)
///   2. `config.org_catalog.<org>.pins.<upstream_name>` (catalogue pin)
///   3. Live `ListOrgTools(<org>)` if the org is subscribed (catalogue latest)
async fn resolve_version(
    svc: &GlobalService,
    org: &str,
    name: &str,
) -> anyhow::Result<String> {
    let cfg = svc.load_user_config().await?;
    let key = format!("{org}/{name}");

    // 1. Explicit pin.
    if let Some(dep) = cfg.dependencies.get(&key) {
        return Ok(dep.version.clone());
    }

    // 2. Catalogue subscription for this org? (Aliases don't matter here —
    //    the qualified ref already names the upstream component.)
    if let Some(cat) = cfg.org_catalog.get(org)
        && cat.enabled
    {
        // 2a. Per-tool pin inside the catalogue, keyed by upstream tool.name.
        //     We don't know the tool.name from `<org>/<name>` directly (the
        //     `name` field is the component name; tool.name may differ via
        //     alias), so fall through to ListOrgTools to learn it.
        // 2b. Live lookup for the latest_version + tool.name.
        let entries = svc.grpc.list_org_tools(org).await.with_context(|| {
            format!("looking up catalogue version for {key}")
        })?;
        for entry in entries {
            if entry.name == name {
                let tool_name = entry
                    .tool
                    .as_ref()
                    .map(|t| t.name.as_str())
                    .unwrap_or(&entry.name);
                if cat.banned.iter().any(|b| b == tool_name) {
                    anyhow::bail!(
                        "{key} is banned in catalogue subscription {org}"
                    );
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
        // `--no-sync=true` should NOT be accepted because the field is a
        // bool flag (SetTrue), not a value-taking argument.
        let res =
            AddHarness::try_parse_from(["forest-global-add", "cuteorg/rg", "--no-sync=true"]);
        assert!(res.is_err(), "expected clap error, got Ok");
    }

    #[test]
    fn format_post_add_sync_handles_many_entries() {
        let out = SyncOutcome {
            created: vec!["a".into(), "b".into()],
            deleted: vec!["c".into(), "d".into(), "e".into()],
        };
        let lines = format_post_add_sync(&out);
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
    fn format_post_add_sync_is_silent_when_no_changes() {
        let out = SyncOutcome {
            created: vec![],
            deleted: vec![],
        };
        assert!(format_post_add_sync(&out).is_empty());
    }

    #[test]
    fn format_post_add_sync_reports_created_and_deleted() {
        let out = SyncOutcome {
            created: vec!["rg".into()],
            deleted: vec!["old".into()],
        };
        let lines = format_post_add_sync(&out);
        assert_eq!(
            lines,
            vec![
                "sync (after add): 1 shim(s) created, 1 deleted".to_string(),
                "  + rg".to_string(),
                "  − old".to_string(),
            ]
        );
    }
}
