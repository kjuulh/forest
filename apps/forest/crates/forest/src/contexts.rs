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
}

impl ContextEntry {
    pub fn new(name: impl Into<String>, server: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            server: server.into(),
            created_at: Some(now_iso8601()),
            default_organisation: None,
        }
    }
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
}
