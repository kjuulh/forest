//! Forest context store — kubectl-style profile switcher.
//!
//! A **context** is a named bundle of `(server URL, auth state)`. One context
//! is active at a time; commands operate against the active one unless
//! overridden by `--context` or `FOREST_CONTEXT`. See TASKS/019-context.md.
//!
//! Layout under `$XDG_DATA_HOME/forest/`:
//!
//! ```text
//! contexts.json              registry: known contexts + active name
//! contexts/
//!   <name>/
//!     user-state.json        per-context auth state (UserState)
//!     user-state.lock        per-context file lock
//! ```

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

const DEFAULT_CONTEXT: &str = "default";
const DEFAULT_SERVER: &str = "http://localhost:4040";

/// Derive a `CUE_REGISTRY` value from a Forest server URL.
///
/// The convention across deployments has been:
///
///   server   `https://forest.understory.sh`
///   →
///   CUE_REGISTRY = `forest.sh=registry.forest.understory.sh,registry.cuelang.org`
///
/// i.e. the `forest.sh` CUE module namespace points at
/// `registry.<server-host>`, with the public CUE registry kept as a
/// fallback so cuelang.org modules still resolve.
///
/// Returns `None` when the server URL can't be parsed or has no host
/// (e.g. someone wrote `forest_server = "localhost:4040"` without a
/// scheme). Callers should fall back to whatever the parent shell
/// already had in `CUE_REGISTRY`.
pub fn derive_cue_registry(server: &str) -> Option<String> {
    // Tolerate URLs missing a scheme by trying the raw value first;
    // if that fails, retry with `https://` prepended. This keeps the
    // helper friendly to half-typed user input without pulling in a
    // proper URL parser.
    let after_scheme = server.split_once("://").map(|(_, rest)| rest).unwrap_or(server);
    let host = after_scheme
        .split('/')
        .next()
        .and_then(|host_port| host_port.split(':').next())
        .filter(|h| !h.is_empty())?;
    Some(format!(
        "forest.sh=registry.{host},registry.cuelang.org"
    ))
}

/// On-disk shape of `contexts.json`. Kept stable; adding fields requires
/// `#[serde(default)]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextsFile {
    pub active: String,
    #[serde(default)]
    pub contexts: Vec<ContextEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextEntry {
    pub name: String,
    pub server: String,
    /// ISO 8601 UTC timestamp. Best-effort; missing on legacy migrations.
    #[serde(default)]
    pub created_at: Option<String>,
    /// Optional default organisation for this context. Currently unused;
    /// reserved for follow-up CLI polish.
    #[serde(default)]
    pub default_organisation: Option<String>,
    /// Optional public-facing forage URL (no trailing slash), used by
    /// `forest auth login --web` to know where to send the browser. See
    /// TASKS/022-device-login.md §1.3. When `None`, the CLI falls back
    /// to `FOREST_WEB_URL`, then a `forest. → forage.` convention.
    #[serde(default)]
    pub web_url: Option<String>,
}

impl ContextEntry {
    pub fn new(name: impl Into<String>, server: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            server: server.into(),
            created_at: Some(now_iso8601()),
            default_organisation: None,
            web_url: None,
        }
    }

    /// Resolve the web URL for this context, walking the fallback chain
    /// documented in TASKS/022-device-login.md §1.3:
    ///   1. explicit `web_url` field
    ///   2. `FOREST_WEB_URL` env var (per-invocation override)
    ///   3. convention: replace first label `forest` → `forage`,
    ///      `https://` enforced. Localhost-by-port maps to forage's
    ///      default dev port (3000).
    ///   4. `None` when no rule applies.
    pub fn resolve_web_url(&self) -> Option<String> {
        if let Some(url) = self.web_url.as_ref().filter(|s| !s.is_empty()) {
            return Some(url.trim_end_matches('/').to_string());
        }
        if let Ok(env_url) = std::env::var("FOREST_WEB_URL") {
            if !env_url.is_empty() {
                return Some(env_url.trim_end_matches('/').to_string());
            }
        }
        derive_web_url_from_server(&self.server)
    }
}

/// Convention: `https://forest.dev.foo` → `https://forage.dev.foo`,
/// `http://localhost:4040` → `http://localhost:3000` (forage's dev port,
/// per `apps/forage/crates/forage-server/src/main.rs:81`).
/// Returns None for shapes we can't safely derive (raw IPs, server URLs
/// that don't start with `forest.`, etc.) — callers should surface a
/// clear "configure web_url" error rather than guess.
fn derive_web_url_from_server(server: &str) -> Option<String> {
    // Manual parse — avoids pulling in the `url` crate just for this.
    // We accept `<scheme>://<host>[:<port>][/<rest>]` and ignore the rest.
    let (scheme, rest) = server.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split('/').next()?;
    // Strip port if present — the convention maps forest's gRPC port
    // (e.g. 4040) to forage's HTTP port (3000), not the same number.
    let host = authority.split(':').next()?;

    // Localhost special case.
    if host == "localhost" || host == "127.0.0.1" {
        return Some(format!("{scheme}://{host}:3000"));
    }

    // Generic case: first label must be "forest"; rewrite to "forage"
    // and force https (forage in production is HTTPS-only).
    let rest_of_host = host.strip_prefix("forest.")?;
    Some(format!("https://forage.{rest_of_host}"))
}

fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Validate a context name against the same regex tool names use
/// (§018 §1a.1).
pub fn validate_name(name: &str) -> Result<()> {
    use forest_manifest::names::{NameError, validate_tool_name};
    match validate_tool_name(name) {
        Ok(()) => Ok(()),
        Err(NameError::Empty) => Err(anyhow!("context name must not be empty")),
        Err(NameError::TooLong { len, max }) => {
            Err(anyhow!("context name is {len} chars; max {max}"))
        }
        Err(NameError::BadFirstChar { ch }) => Err(anyhow!(
            "context name must start with a letter; got {ch:?}"
        )),
        Err(NameError::BadChar { ch, position }) => Err(anyhow!(
            "context name contains illegal char {ch:?} at position {position}"
        )),
        Err(NameError::ContainsDotDot) => {
            Err(anyhow!("context name must not contain `..`"))
        }
    }
}

/// Filesystem-backed context registry.
#[derive(Debug, Clone)]
pub struct ContextStore {
    data_dir: PathBuf,
}

impl ContextStore {
    /// Construct using XDG defaults (`$XDG_DATA_HOME/forest/` or
    /// `~/.local/share/forest/`). Tests prefer `with_root` for isolation.
    pub fn from_env() -> Result<Self> {
        let data_dir = dirs::data_local_dir()
            .ok_or_else(|| anyhow!("unable to resolve XDG data dir"))?
            .join("forest");
        Ok(Self { data_dir })
    }

    /// Test-friendly constructor pointing at a specific data root.
    pub fn with_root(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    pub fn contexts_file(&self) -> PathBuf {
        self.data_dir.join("contexts.json")
    }

    /// Per-context directory holding `user-state.json` + `user-state.lock`.
    pub fn context_dir(&self, name: &str) -> PathBuf {
        self.data_dir.join("contexts").join(name)
    }

    /// Path the existing `UserStateLoader` should target for `name`.
    pub fn user_state_dir(&self, name: &str) -> PathBuf {
        self.context_dir(name)
    }

    /// Load the registry, migrating from a legacy single user-state.json if
    /// necessary. Always returns a valid file; bootstraps `default` on first
    /// run.
    pub fn load_or_bootstrap(&self) -> Result<ContextsFile> {
        if let Some(file) = self.read_file()? {
            return Ok(file);
        }

        // Bootstrap. Two flavours:
        //   - Legacy single user-state.json present → adopt as `default`.
        //   - Nothing at all → fresh empty `default`.
        let legacy = self.data_dir.join("user-state.json");
        let server_default = std::env::var("FOREST_SERVER")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_SERVER.to_string());

        let entry = ContextEntry::new(DEFAULT_CONTEXT, server_default);

        if legacy.exists() {
            // Move the legacy file into contexts/default/user-state.json
            // *before* writing contexts.json so a crash mid-migration is recoverable.
            let target = self.context_dir(DEFAULT_CONTEXT).join("user-state.json");
            std::fs::create_dir_all(target.parent().unwrap())
                .context("creating contexts/default dir")?;
            std::fs::rename(&legacy, &target).with_context(|| {
                format!("migrating legacy {} -> {}", legacy.display(), target.display())
            })?;
            // Also move the .lock if present, best-effort.
            let legacy_lock = self.data_dir.join("user-state.lock");
            if legacy_lock.exists() {
                let _ = std::fs::rename(
                    &legacy_lock,
                    self.context_dir(DEFAULT_CONTEXT).join("user-state.lock"),
                );
            }
            eprintln!(
                "forest: migrated legacy user-state.json into context 'default'"
            );
        }

        let file = ContextsFile {
            active: DEFAULT_CONTEXT.to_string(),
            contexts: vec![entry],
        };
        self.write_file(&file)?;
        Ok(file)
    }

    fn read_file(&self) -> Result<Option<ContextsFile>> {
        let path = self.contexts_file();
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let file: ContextsFile = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(Some(file))
    }

    fn write_file(&self, file: &ContextsFile) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("creating {}", self.data_dir.display()))?;
        let body = serde_json::to_vec_pretty(file)?;
        let path = self.contexts_file();
        // Atomic write: tempfile in same dir + rename.
        let rand: u64 = rand::random();
        let tmp = self.data_dir.join(format!(".contexts.tmp.{rand:016x}"));
        std::fs::write(&tmp, &body)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }

    pub fn list(&self) -> Result<ContextsFile> {
        self.load_or_bootstrap()
    }

    pub fn active(&self) -> Result<ContextEntry> {
        let file = self.load_or_bootstrap()?;
        file.contexts
            .iter()
            .find(|c| c.name == file.active)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "active context '{}' is missing from contexts list",
                    file.active
                )
            })
    }

    /// Resolve a context by name, or the active one if `name` is None,
    /// *without* triggering the first-run bootstrap. Returns `Ok(None)`
    /// when no `contexts.json` exists on disk.
    ///
    /// This is the right entry point for callers that want to *read*
    /// the context state (banners, env overlays) without side-effects.
    /// Use [`Self::resolve`] when bootstrapping is desired (typical
    /// command execution).
    pub fn try_resolve(&self, name: Option<&str>) -> Result<Option<ContextEntry>> {
        let Some(file) = self.read_file()? else {
            return Ok(None);
        };
        let want = name.unwrap_or(&file.active);
        Ok(file.contexts.iter().find(|c| c.name == want).cloned())
    }

    /// Resolve a context by name, or the active one if `name` is None.
    pub fn resolve(&self, name: Option<&str>) -> Result<ContextEntry> {
        let file = self.load_or_bootstrap()?;
        let want = name.unwrap_or(&file.active);
        file.contexts
            .iter()
            .find(|c| c.name == want)
            .cloned()
            .ok_or_else(|| {
                let names: Vec<&str> =
                    file.contexts.iter().map(|c| c.name.as_str()).collect();
                anyhow!("context '{want}' not found. known: [{}]", names.join(", "))
            })
    }

    pub fn use_context(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        let mut file = self.load_or_bootstrap()?;
        if !file.contexts.iter().any(|c| c.name == name) {
            let names: Vec<&str> =
                file.contexts.iter().map(|c| c.name.as_str()).collect();
            anyhow::bail!("context '{name}' not found. known: [{}]", names.join(", "));
        }
        file.active = name.to_string();
        self.write_file(&file)?;
        Ok(())
    }

    pub fn create(&self, name: &str, server: &str, switch_to: bool) -> Result<ContextEntry> {
        validate_name(name)?;
        let mut file = self.load_or_bootstrap()?;
        if file.contexts.iter().any(|c| c.name == name) {
            anyhow::bail!("context '{name}' already exists");
        }
        let entry = ContextEntry::new(name, server);
        file.contexts.push(entry.clone());
        if switch_to {
            file.active = name.to_string();
        }
        self.write_file(&file)?;
        std::fs::create_dir_all(self.context_dir(name))
            .with_context(|| format!("creating context dir for {name}"))?;
        Ok(entry)
    }

    pub fn delete(&self, name: &str, force: bool, keep_data: bool) -> Result<()> {
        validate_name(name)?;
        let mut file = self.load_or_bootstrap()?;
        if name == file.active && !force {
            anyhow::bail!(
                "context '{name}' is currently active; switch with `forest context use <other>` first, or pass --force"
            );
        }
        let before = file.contexts.len();
        file.contexts.retain(|c| c.name != name);
        if file.contexts.len() == before {
            anyhow::bail!("context '{name}' not found");
        }
        // If we deleted the active context with --force, pick a fallback active.
        if name == file.active {
            file.active = file
                .contexts
                .first()
                .map(|c| c.name.clone())
                .unwrap_or_else(|| DEFAULT_CONTEXT.to_string());
        }
        self.write_file(&file)?;
        if !keep_data && self.context_dir(name).exists() {
            std::fs::remove_dir_all(self.context_dir(name)).with_context(|| {
                format!("removing context dir for {name}")
            })?;
        }
        Ok(())
    }

    pub fn rename(&self, old: &str, new: &str) -> Result<()> {
        validate_name(new)?;
        let mut file = self.load_or_bootstrap()?;
        if file.contexts.iter().any(|c| c.name == new) {
            anyhow::bail!("context '{new}' already exists");
        }
        let entry = file
            .contexts
            .iter_mut()
            .find(|c| c.name == old)
            .ok_or_else(|| anyhow!("context '{old}' not found"))?;
        entry.name = new.to_string();
        let was_active = file.active == old;
        if was_active {
            file.active = new.to_string();
        }
        self.write_file(&file)?;
        let old_dir = self.context_dir(old);
        let new_dir = self.context_dir(new);
        if old_dir.exists() {
            std::fs::rename(&old_dir, &new_dir).with_context(|| {
                format!("moving {} -> {}", old_dir.display(), new_dir.display())
            })?;
        }
        Ok(())
    }

    /// Install-time provisioning: idempotently register a context with
    /// "first-run takes the default" semantics.
    ///
    /// - If `contexts.json` doesn't yet exist, the new entry becomes the
    ///   sole context and is set active.
    /// - If it does exist and the name is new, the entry is appended;
    ///   the previously-active context stays active.
    /// - If a context with that name already exists, its server URL is
    ///   updated in place (so re-running `install.sh` with a refreshed
    ///   FOREST_PROFILE picks up the change without manual cleanup).
    ///
    /// Differs from [`Self::create`] in two ways: it never errors on
    /// duplicate names, and the "should I become active" decision is
    /// derived from the disk state rather than a caller-supplied flag.
    pub fn provision(&self, name: &str, server: &str) -> Result<ContextEntry> {
        validate_name(name)?;
        match self.read_file()? {
            None => {
                // First run: skip the load_or_bootstrap default and
                // make the provisioned context the only one.
                let entry = ContextEntry::new(name, server);
                let file = ContextsFile {
                    active: name.to_string(),
                    contexts: vec![entry.clone()],
                };
                std::fs::create_dir_all(self.context_dir(name))
                    .with_context(|| format!("creating context dir for {name}"))?;
                self.write_file(&file)?;
                Ok(entry)
            }
            Some(mut file) => {
                if let Some(existing) = file.contexts.iter_mut().find(|c| c.name == name) {
                    existing.server = server.to_string();
                    let cloned = existing.clone();
                    self.write_file(&file)?;
                    Ok(cloned)
                } else {
                    let entry = ContextEntry::new(name, server);
                    file.contexts.push(entry.clone());
                    std::fs::create_dir_all(self.context_dir(name))
                        .with_context(|| format!("creating context dir for {name}"))?;
                    self.write_file(&file)?;
                    Ok(entry)
                }
            }
        }
    }

    pub fn set_server(&self, name: &str, server: &str) -> Result<()> {
        let mut file = self.load_or_bootstrap()?;
        let entry = file
            .contexts
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or_else(|| anyhow!("context '{name}' not found"))?;
        entry.server = server.to_string();
        self.write_file(&file)?;
        Ok(())
    }

    /// Set (or clear, with `web_url=None`) the web_url for a context.
    /// Used by `forest context set-web-url` and by FOREST_PROFILE
    /// provisioning when a `web=` value is supplied.
    pub fn set_web_url(&self, name: &str, web_url: Option<&str>) -> Result<()> {
        let mut file = self.load_or_bootstrap()?;
        let entry = file
            .contexts
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or_else(|| anyhow!("context '{name}' not found"))?;
        entry.web_url = web_url.map(|s| s.trim_end_matches('/').to_string());
        self.write_file(&file)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store(td: &TempDir) -> ContextStore {
        ContextStore::with_root(td.path().to_path_buf())
    }

    #[test]
    fn bootstrap_creates_default_context() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        let file = s.load_or_bootstrap().unwrap();
        assert_eq!(file.active, "default");
        assert_eq!(file.contexts.len(), 1);
        assert_eq!(file.contexts[0].name, "default");
    }

    #[test]
    fn bootstrap_migrates_legacy_user_state() {
        let td = TempDir::new().unwrap();
        let legacy = td.path().join("forest").join("user-state.json");
        // The store is rooted at td/forest, so simulate the legacy file living there.
        let data_dir = td.path().join("forest");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(&legacy, br#"{"user_id":"x","username":"u","emails":[],"access_token":"a","refresh_access":"r"}"#).unwrap();

        let s = ContextStore::with_root(data_dir.clone());
        let file = s.load_or_bootstrap().unwrap();
        assert_eq!(file.contexts.len(), 1);
        assert!(!legacy.exists(), "legacy file must be moved away");
        let migrated = data_dir.join("contexts/default/user-state.json");
        assert!(migrated.exists(), "must land at {}", migrated.display());
    }

    #[test]
    fn create_and_use_context() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        s.load_or_bootstrap().unwrap();
        s.create("prod", "https://prod.example.com", false).unwrap();
        let file = s.list().unwrap();
        assert_eq!(file.active, "default");
        assert_eq!(file.contexts.len(), 2);
        s.use_context("prod").unwrap();
        assert_eq!(s.active().unwrap().name, "prod");
    }

    #[test]
    fn create_with_use_switches_active() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        s.create("localdev", "http://localhost:4040", true).unwrap();
        assert_eq!(s.active().unwrap().name, "localdev");
    }

    #[test]
    fn duplicate_create_rejected() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        s.create("dup", "http://x", false).unwrap();
        let err = s.create("dup", "http://y", false).unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn delete_active_requires_force() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        s.load_or_bootstrap().unwrap();
        let err = s.delete("default", false, false).unwrap_err();
        assert!(err.to_string().contains("currently active"));
        s.delete("default", true, true).unwrap();
    }

    #[test]
    fn rename_preserves_active_marker() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        s.load_or_bootstrap().unwrap();
        s.rename("default", "main").unwrap();
        assert_eq!(s.active().unwrap().name, "main");
    }

    #[test]
    fn rename_moves_context_dir() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        s.create("a", "http://x", false).unwrap();
        // Drop a file inside the context dir.
        std::fs::write(s.context_dir("a").join("user-state.json"), "{}").unwrap();
        s.rename("a", "b").unwrap();
        assert!(!s.context_dir("a").exists());
        assert!(s.context_dir("b").join("user-state.json").exists());
    }

    #[test]
    fn set_server_updates_url() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        s.create("env", "http://old", false).unwrap();
        s.set_server("env", "http://new").unwrap();
        let entry = s.resolve(Some("env")).unwrap();
        assert_eq!(entry.server, "http://new");
    }

    // ── web_url resolution chain (TASKS/022-device-login.md §1.3) ────

    #[test]
    fn resolve_web_url_uses_explicit_field_first() {
        let mut e = ContextEntry::new("p", "https://forest.dev.example.com");
        e.web_url = Some("https://override.example".into());
        assert_eq!(
            e.resolve_web_url().as_deref(),
            Some("https://override.example")
        );
    }

    #[test]
    fn resolve_web_url_derives_from_forest_subdomain() {
        let e = ContextEntry::new("p", "https://forest.dev.understory.sh");
        assert_eq!(
            e.resolve_web_url().as_deref(),
            Some("https://forage.dev.understory.sh")
        );
    }

    #[test]
    fn resolve_web_url_derives_localhost_to_port_3000() {
        let e = ContextEntry::new("p", "http://localhost:4040");
        assert_eq!(
            e.resolve_web_url().as_deref(),
            Some("http://localhost:3000")
        );
    }

    #[test]
    fn resolve_web_url_returns_none_for_unguessable_host() {
        // A raw IP or a non-`forest.` hostname has no convention; the CLI
        // must surface a clear error rather than silently guess.
        let e = ContextEntry::new("p", "https://10.0.0.1:4040");
        assert_eq!(e.resolve_web_url(), None);
    }

    #[test]
    fn resolve_web_url_returns_none_for_non_http_scheme() {
        let e = ContextEntry::new("p", "grpc://forest.example.com");
        assert_eq!(e.resolve_web_url(), None);
    }

    #[test]
    fn set_web_url_persists_value() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        s.create("c", "https://forest.example.com", false).unwrap();
        s.set_web_url("c", Some("https://forage.example.com"))
            .unwrap();
        let entry = s.resolve(Some("c")).unwrap();
        assert_eq!(
            entry.web_url.as_deref(),
            Some("https://forage.example.com")
        );
    }

    #[test]
    fn set_web_url_can_clear() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        s.create("c", "https://forest.example.com", false).unwrap();
        s.set_web_url("c", Some("https://forage.example.com"))
            .unwrap();
        s.set_web_url("c", None).unwrap();
        let entry = s.resolve(Some("c")).unwrap();
        assert!(entry.web_url.is_none());
    }

    #[test]
    fn set_web_url_strips_trailing_slash() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        s.create("c", "https://forest.example.com", false).unwrap();
        s.set_web_url("c", Some("https://forage.example.com/"))
            .unwrap();
        let entry = s.resolve(Some("c")).unwrap();
        assert_eq!(
            entry.web_url.as_deref(),
            Some("https://forage.example.com")
        );
    }

    #[test]
    fn resolve_unknown_lists_known() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        s.create("a", "http://x", false).unwrap();
        let err = s.resolve(Some("missing")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not found"), "{msg}");
        assert!(msg.contains("a") && msg.contains("default"), "should list known: {msg}");
    }

    #[test]
    fn name_validation_rejects_bad_inputs() {
        assert!(validate_name("").is_err());
        assert!(validate_name("1bad").is_err());
        assert!(validate_name("a..b").is_err());
        assert!(validate_name("ok-name").is_ok());
    }

    // ── provision ────────────────────────────────────────────────

    #[test]
    fn provision_on_empty_disk_creates_sole_active_context() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        // No bootstrap was triggered yet — `contexts.json` does not exist.
        assert!(!s.contexts_file().exists());

        let entry = s
            .provision("understory-prod", "https://forest.understory.sh")
            .unwrap();
        assert_eq!(entry.name, "understory-prod");
        assert_eq!(entry.server, "https://forest.understory.sh");

        // The provisioned context is the only one AND is active —
        // the bootstrap default never got created.
        let file = s.list().unwrap();
        assert_eq!(file.contexts.len(), 1);
        assert_eq!(file.contexts[0].name, "understory-prod");
        assert_eq!(file.active, "understory-prod");
    }

    #[test]
    fn provision_does_not_change_active_when_other_contexts_exist() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        // Bootstrap a `default` so the file exists with active=default.
        let _ = s.load_or_bootstrap().unwrap();
        assert_eq!(s.active().unwrap().name, "default");

        s.provision("understory-prod", "https://forest.understory.sh")
            .unwrap();

        let file = s.list().unwrap();
        assert_eq!(file.contexts.len(), 2);
        assert_eq!(
            file.active, "default",
            "the provisioned context must NOT steal the active slot"
        );
    }

    #[test]
    fn provision_is_idempotent_and_updates_server() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        s.provision("prod", "https://old.example.com").unwrap();
        s.provision("prod", "https://new.example.com").unwrap();
        let entry = s.resolve(Some("prod")).unwrap();
        assert_eq!(entry.server, "https://new.example.com");
        // Only one entry: re-provisioning didn't duplicate.
        assert_eq!(s.list().unwrap().contexts.len(), 1);
    }

    // ── derive_cue_registry ──────────────────────────────────────

    #[test]
    fn derive_cue_registry_pulls_host_and_prepends_registry() {
        assert_eq!(
            derive_cue_registry("https://forest.understory.sh"),
            Some("forest.sh=registry.forest.understory.sh,registry.cuelang.org".into())
        );
    }

    #[test]
    fn derive_cue_registry_handles_port_and_path() {
        assert_eq!(
            derive_cue_registry("https://forest.example.com:4040/api"),
            Some("forest.sh=registry.forest.example.com,registry.cuelang.org".into())
        );
    }

    #[test]
    fn derive_cue_registry_tolerates_missing_scheme() {
        assert_eq!(
            derive_cue_registry("forest.example.com"),
            Some("forest.sh=registry.forest.example.com,registry.cuelang.org".into())
        );
    }

    #[test]
    fn derive_cue_registry_returns_none_for_no_host() {
        assert_eq!(derive_cue_registry(""), None);
        // Bare port with no host: nothing to derive from.
        assert_eq!(derive_cue_registry(":4040"), None);
        // Just `/path` with no host either.
        assert_eq!(derive_cue_registry("/api"), None);
    }
}
