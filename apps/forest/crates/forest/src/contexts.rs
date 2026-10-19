use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

const DEFAULT_CONTEXT: &str = "default";
const DEFAULT_SERVER: &str = "http://localhost:4040";

pub fn derive_cue_registry(server: &str) -> Option<String> {
    let after_scheme = server
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(server);
    let host = after_scheme
        .split('/')
        .next()
        .and_then(|host_port| host_port.split(':').next())
        .filter(|h| !h.is_empty())?;
    Some(format!("forest.sh=registry.{host},registry.cuelang.org"))
}

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
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub default_organisation: Option<String>,
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

fn derive_web_url_from_server(server: &str) -> Option<String> {
    let (scheme, rest) = server.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split('/').next()?;
    let host = authority.split(':').next()?;

    if host == "localhost" || host == "127.0.0.1" {
        return Some(format!("{scheme}://{host}:3000"));
    }

    let rest_of_host = host.strip_prefix("api.")?;
    Some(format!("https://{rest_of_host}"))
}

fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn validate_name(name: &str) -> Result<()> {
    use forest_manifest::names::{NameError, validate_tool_name};
    match validate_tool_name(name) {
        Ok(()) => Ok(()),
        Err(NameError::Empty) => Err(anyhow!("context name must not be empty")),
        Err(NameError::TooLong { len, max }) => {
            Err(anyhow!("context name is {len} chars; max {max}"))
        }
        Err(NameError::BadFirstChar { ch }) => {
            Err(anyhow!("context name must start with a letter; got {ch:?}"))
        }
        Err(NameError::BadChar { ch, position }) => Err(anyhow!(
            "context name contains illegal char {ch:?} at position {position}"
        )),
        Err(NameError::ContainsDotDot) => Err(anyhow!("context name must not contain `..`")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerTarget {
    pub label: String,
    pub server: String,
}

pub fn resolve_server_target(
    forest_server: Option<&str>,
    context: Option<&str>,
    store: &ContextStore,
) -> Result<ServerTarget> {
    if let Some(server) = forest_server.filter(|s| !s.is_empty()) {
        return Ok(ServerTarget {
            label: server.to_string(),
            server: server.to_string(),
        });
    }
    let entry = store.resolve(context)?;
    Ok(ServerTarget {
        label: entry.name,
        server: entry.server,
    })
}

#[derive(Debug, Clone)]
pub struct ContextStore {
    data_dir: PathBuf,
}

impl ContextStore {
    pub fn from_env() -> Result<Self> {
        let data_dir = dirs::data_local_dir()
            .ok_or_else(|| anyhow!("unable to resolve XDG data dir"))?
            .join("forest");
        Ok(Self { data_dir })
    }

    pub fn with_root(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    pub fn contexts_file(&self) -> PathBuf {
        self.data_dir.join("contexts.json")
    }

    pub fn context_dir(&self, name: &str) -> PathBuf {
        self.data_dir.join("contexts").join(name)
    }

    pub fn user_state_dir(&self, name: &str) -> PathBuf {
        self.context_dir(name)
    }

    pub fn load_or_bootstrap(&self) -> Result<ContextsFile> {
        if let Some(file) = self.read_file()? {
            return Ok(file);
        }

        let legacy = self.data_dir.join("user-state.json");
        let server_default = std::env::var("FOREST_SERVER")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_SERVER.to_string());

        let entry = ContextEntry::new(DEFAULT_CONTEXT, server_default);

        if legacy.exists() {
            let target = self.context_dir(DEFAULT_CONTEXT).join("user-state.json");
            std::fs::create_dir_all(target.parent().unwrap())
                .context("creating contexts/default dir")?;
            std::fs::rename(&legacy, &target).with_context(|| {
                format!(
                    "migrating legacy {} -> {}",
                    legacy.display(),
                    target.display()
                )
            })?;
            let legacy_lock = self.data_dir.join("user-state.lock");
            if legacy_lock.exists() {
                let _ = std::fs::rename(
                    &legacy_lock,
                    self.context_dir(DEFAULT_CONTEXT).join("user-state.lock"),
                );
            }
            eprintln!("forest: migrated legacy user-state.json into context 'default'");
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
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let file: ContextsFile = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(Some(file))
    }

    fn write_file(&self, file: &ContextsFile) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("creating {}", self.data_dir.display()))?;
        let body = serde_json::to_vec_pretty(file)?;
        let path = self.contexts_file();
        let rand: u64 = rand::random();
        let tmp = self.data_dir.join(format!(".contexts.tmp.{rand:016x}"));
        std::fs::write(&tmp, &body).with_context(|| format!("writing {}", tmp.display()))?;
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

    pub fn try_resolve(&self, name: Option<&str>) -> Result<Option<ContextEntry>> {
        let Some(file) = self.read_file()? else {
            return Ok(None);
        };
        let want = name.unwrap_or(&file.active);
        Ok(file.contexts.iter().find(|c| c.name == want).cloned())
    }

    pub fn resolve(&self, name: Option<&str>) -> Result<ContextEntry> {
        let file = self.load_or_bootstrap()?;
        let want = name.unwrap_or(&file.active);
        file.contexts
            .iter()
            .find(|c| c.name == want)
            .cloned()
            .ok_or_else(|| {
                let names: Vec<&str> = file.contexts.iter().map(|c| c.name.as_str()).collect();
                anyhow!("context '{want}' not found. known: [{}]", names.join(", "))
            })
    }

    pub fn use_context(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        let mut file = self.load_or_bootstrap()?;
        if !file.contexts.iter().any(|c| c.name == name) {
            let names: Vec<&str> = file.contexts.iter().map(|c| c.name.as_str()).collect();
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
        if name == file.active {
            file.active = file
                .contexts
                .first()
                .map(|c| c.name.clone())
                .unwrap_or_else(|| DEFAULT_CONTEXT.to_string());
        }
        self.write_file(&file)?;
        if !keep_data && self.context_dir(name).exists() {
            std::fs::remove_dir_all(self.context_dir(name))
                .with_context(|| format!("removing context dir for {name}"))?;
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

    pub fn provision(&self, name: &str, server: &str) -> Result<ContextEntry> {
        validate_name(name)?;
        match self.read_file()? {
            None => {
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

    fn two_contexts(td: &TempDir) -> ContextStore {
        let s = store(td);
        s.create("home", "https://api.home.example", true).unwrap();
        s.create("work", "https://api.work.example", false).unwrap();
        s
    }

    #[test]
    fn server_target_is_the_active_context_by_default() {
        let td = TempDir::new().unwrap();
        let target = resolve_server_target(None, None, &two_contexts(&td)).unwrap();
        assert_eq!(target.label, "home");
        assert_eq!(target.server, "https://api.home.example");
    }

    #[test]
    fn a_named_context_wins_over_the_active_one() {
        let td = TempDir::new().unwrap();
        let target = resolve_server_target(None, Some("work"), &two_contexts(&td)).unwrap();
        assert_eq!(target.label, "work");
        assert_eq!(target.server, "https://api.work.example");
    }

    #[test]
    fn an_explicit_server_wins_over_any_context() {
        let td = TempDir::new().unwrap();
        let target = resolve_server_target(
            Some("https://api.other.example"),
            Some("work"),
            &two_contexts(&td),
        )
        .unwrap();
        assert_eq!(target.server, "https://api.other.example");
        assert_eq!(target.label, "https://api.other.example");
    }

    #[test]
    fn an_empty_server_falls_through_to_the_context() {
        let td = TempDir::new().unwrap();
        let target = resolve_server_target(Some(""), Some("work"), &two_contexts(&td)).unwrap();
        assert_eq!(target.label, "work");
    }

    #[test]
    fn an_unknown_context_is_an_error_not_the_active_one() {
        let td = TempDir::new().unwrap();
        assert!(resolve_server_target(None, Some("nope"), &two_contexts(&td)).is_err());
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
    fn resolve_web_url_uses_explicit_field_first() {
        let mut e = ContextEntry::new("p", "https://forest.dev.example.com");
        e.web_url = Some("https://override.example".into());
        assert_eq!(
            e.resolve_web_url().as_deref(),
            Some("https://override.example")
        );
    }

    #[test]
    fn resolve_web_url_derives_from_api_prefix() {
        let e = ContextEntry::new("p", "https://api.forest.dev.understory.sh");
        assert_eq!(
            e.resolve_web_url().as_deref(),
            Some("https://forest.dev.understory.sh")
        );
    }

    #[test]
    fn resolve_web_url_derives_production_api_to_bare_forest() {
        let e = ContextEntry::new("p", "https://api.forest.understory.sh");
        assert_eq!(
            e.resolve_web_url().as_deref(),
            Some("https://forest.understory.sh")
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
    fn resolve_web_url_returns_none_when_api_prefix_missing() {
        let e = ContextEntry::new("p", "https://forest.understory.sh");
        assert_eq!(e.resolve_web_url(), None);
    }

    #[test]
    fn resolve_web_url_returns_none_for_raw_ip() {
        let e = ContextEntry::new("p", "https://10.0.0.1:4040");
        assert_eq!(e.resolve_web_url(), None);
    }

    #[test]
    fn resolve_web_url_returns_none_for_non_http_scheme() {
        let e = ContextEntry::new("p", "grpc://api.forest.example.com");
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
        assert_eq!(entry.web_url.as_deref(), Some("https://forage.example.com"));
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
        assert_eq!(entry.web_url.as_deref(), Some("https://forage.example.com"));
    }

    #[test]
    fn resolve_unknown_lists_known() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        s.create("a", "http://x", false).unwrap();
        let err = s.resolve(Some("missing")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not found"), "{msg}");
        assert!(
            msg.contains("a") && msg.contains("default"),
            "should list known: {msg}"
        );
    }

    #[test]
    fn name_validation_rejects_bad_inputs() {
        assert!(validate_name("").is_err());
        assert!(validate_name("1bad").is_err());
        assert!(validate_name("a..b").is_err());
        assert!(validate_name("ok-name").is_ok());
    }

    #[test]
    fn provision_on_empty_disk_creates_sole_active_context() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        assert!(!s.contexts_file().exists());

        let entry = s
            .provision("understory-prod", "https://api.forest.understory.sh")
            .unwrap();
        assert_eq!(entry.name, "understory-prod");
        assert_eq!(entry.server, "https://api.forest.understory.sh");

        let file = s.list().unwrap();
        assert_eq!(file.contexts.len(), 1);
        assert_eq!(file.contexts[0].name, "understory-prod");
        assert_eq!(file.active, "understory-prod");
    }

    #[test]
    fn provision_does_not_change_active_when_other_contexts_exist() {
        let td = TempDir::new().unwrap();
        let s = store(&td);
        let _ = s.load_or_bootstrap().unwrap();
        assert_eq!(s.active().unwrap().name, "default");

        s.provision("understory-prod", "https://api.forest.understory.sh")
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
        assert_eq!(s.list().unwrap().contexts.len(), 1);
    }

    #[test]
    fn derive_cue_registry_pulls_host_and_prepends_registry() {
        assert_eq!(
            derive_cue_registry("https://api.forest.understory.sh"),
            Some("forest.sh=registry.api.forest.understory.sh,registry.cuelang.org".into())
        );
    }

    #[test]
    fn derive_cue_registry_handles_port_and_path() {
        assert_eq!(
            derive_cue_registry("https://api.forest.example.com:4040/api"),
            Some("forest.sh=registry.api.forest.example.com,registry.cuelang.org".into())
        );
    }

    #[test]
    fn derive_cue_registry_tolerates_missing_scheme() {
        assert_eq!(
            derive_cue_registry("api.forest.example.com"),
            Some("forest.sh=registry.api.forest.example.com,registry.cuelang.org".into())
        );
    }

    #[test]
    fn derive_cue_registry_returns_none_for_no_host() {
        assert_eq!(derive_cue_registry(""), None);
        assert_eq!(derive_cue_registry(":4040"), None);
        assert_eq!(derive_cue_registry("/api"), None);
    }
}
