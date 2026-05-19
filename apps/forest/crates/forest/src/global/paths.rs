//! Resolver for XDG-style filesystem locations.
//!
//! Centralises the three paths Forest's global-tools layer touches:
//! - `~/.config/forest/forest.cue` (user config — `$XDG_CONFIG_HOME`)
//! - `~/.local/state/forest/forest.lock` (lockfile — `$XDG_STATE_HOME`)
//! - `~/.cache/forest/...` (cache: `bin/<sha>` for content-addressed binaries,
//!   `global/shims/` for shims — `$XDG_CACHE_HOME`)

use std::path::PathBuf;

const APP_DIR: &str = "forest";
const CONFIG_FILE: &str = "forest.cue";
const LEGACY_CONFIG_FILE: &str = "forest.toml";
const LOCKFILE: &str = "forest.lock";
const LOCK_GUARD_FILE: &str = ".lock";

/// Bundle of resolved locations. Constructed once per CLI invocation;
/// cloned by reference into anything that needs paths.
#[derive(Debug, Clone)]
pub struct GlobalPaths {
    config_dir: PathBuf,
    state_dir: PathBuf,
    cache_dir: PathBuf,
}

impl GlobalPaths {
    /// Resolve from the user's XDG environment (or defaults).
    pub fn from_env() -> anyhow::Result<Self> {
        let config_dir = xdg_config_home()?.join(APP_DIR);
        let state_dir = xdg_state_home()?.join(APP_DIR);
        let cache_dir = xdg_cache_home()?.join(APP_DIR);
        Ok(Self {
            config_dir,
            state_dir,
            cache_dir,
        })
    }

    /// Explicit constructor for tests.
    pub fn with_roots(config_dir: PathBuf, state_dir: PathBuf, cache_dir: PathBuf) -> Self {
        Self {
            config_dir,
            state_dir,
            cache_dir,
        }
    }

    pub fn config_dir(&self) -> &PathBuf {
        &self.config_dir
    }

    pub fn state_dir(&self) -> &PathBuf {
        &self.state_dir
    }

    pub fn cache_dir(&self) -> &PathBuf {
        &self.cache_dir
    }

    pub fn user_config_cue(&self) -> PathBuf {
        self.config_dir.join(CONFIG_FILE)
    }

    pub fn legacy_user_config_toml(&self) -> PathBuf {
        self.config_dir.join(LEGACY_CONFIG_FILE)
    }

    pub fn lockfile(&self) -> PathBuf {
        self.state_dir.join(LOCKFILE)
    }

    pub fn write_lock_guard(&self) -> PathBuf {
        self.config_dir.join(LOCK_GUARD_FILE)
    }

    pub fn shims_dir(&self) -> PathBuf {
        self.cache_dir.join("global").join("shims")
    }

    pub fn binary_cache_dir(&self) -> PathBuf {
        self.cache_dir.join("components").join("bin")
    }

    pub fn cached_binary(&self, sha: &str) -> PathBuf {
        // sha may be "sha256:hex" or "hex" — strip the prefix so the on-disk
        // name is always just the hex digest.
        let hex = sha.strip_prefix("sha256:").unwrap_or(sha);
        self.binary_cache_dir().join(hex)
    }
}

fn xdg_config_home() -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var("XDG_CONFIG_HOME")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home_dir is unset"))?;
    Ok(home.join(".config"))
}

fn xdg_state_home() -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var("XDG_STATE_HOME")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home_dir is unset"))?;
    Ok(home.join(".local").join("state"))
}

fn xdg_cache_home() -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var("XDG_CACHE_HOME")
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home_dir is unset"))?;
    Ok(home.join(".cache"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixed() -> GlobalPaths {
        GlobalPaths::with_roots(
            PathBuf::from("/cfg/forest"),
            PathBuf::from("/state/forest"),
            PathBuf::from("/cache/forest"),
        )
    }

    #[test]
    fn user_config_cue_lives_under_config_dir() {
        assert_eq!(fixed().user_config_cue(), PathBuf::from("/cfg/forest/forest.cue"));
    }

    #[test]
    fn lockfile_lives_under_state_dir() {
        // §1a.4 — XDG_STATE_HOME, not config, not cache.
        assert_eq!(fixed().lockfile(), PathBuf::from("/state/forest/forest.lock"));
    }

    #[test]
    fn shims_dir_lives_under_cache_dir() {
        assert_eq!(
            fixed().shims_dir(),
            PathBuf::from("/cache/forest/global/shims"),
        );
    }

    #[test]
    fn binary_cache_lives_under_cache_dir() {
        assert_eq!(
            fixed().binary_cache_dir(),
            PathBuf::from("/cache/forest/components/bin"),
        );
    }

    #[test]
    fn cached_binary_strips_sha256_prefix() {
        assert_eq!(
            fixed().cached_binary("sha256:abc123"),
            PathBuf::from("/cache/forest/components/bin/abc123"),
        );
        assert_eq!(
            fixed().cached_binary("abc123"),
            PathBuf::from("/cache/forest/components/bin/abc123"),
        );
    }

    #[test]
    fn write_lock_guard_lives_in_config_dir() {
        assert_eq!(
            fixed().write_lock_guard(),
            PathBuf::from("/cfg/forest/.lock"),
        );
    }
}
