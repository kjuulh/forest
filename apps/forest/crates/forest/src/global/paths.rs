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

    /// The content-addressed **directory** for one binary's bytes
    /// (`bin/<hex>`). The binaries themselves live inside it, named after the
    /// component (DATA-510) — see [`Self::cached_binary`].
    ///
    /// One hash can be known under several names (an alias, or two components
    /// whose artifacts dedupe to the same bytes), so the directory holds a
    /// hard-linked entry per name rather than a single file.
    pub fn cached_binary_dir(&self, sha: &str) -> PathBuf {
        // sha may be "sha256:hex" or "hex" — strip the prefix so the on-disk
        // name is always just the hex digest.
        let hex = sha.strip_prefix("sha256:").unwrap_or(sha);
        self.binary_cache_dir().join(hex)
    }

    /// The executable for `sha`, materialised under its real name:
    /// `bin/<hex>/<bin_name>`.
    ///
    /// Naming the file after the component is the whole point — `exec`ing this
    /// path makes the child's `argv[0]` basename the component name instead of
    /// a sha256 digest, which multi-call binaries and usage text depend on.
    pub fn cached_binary(&self, sha: &str, bin_name: &str) -> PathBuf {
        self.cached_binary_dir(sha).join(bin_name)
    }

    /// Pre-DATA-510 location: the content-addressed path *was* the executable.
    /// Identical to [`Self::cached_binary_dir`] — kept as a distinct name so
    /// the migration code reads unambiguously at its call sites.
    pub fn legacy_cached_binary_file(&self, sha: &str) -> PathBuf {
        self.cached_binary_dir(sha)
    }

    /// Per-(org, name, version) directory for the `include` block shipped
    /// beside a tool's binary (TASKS/023). Keyed by version — not by binary
    /// sha — so it loads on the offline warm path and never collides when two
    /// versions dedupe to the same binary. Future include members (e.g. files)
    /// live as siblings in this dir.
    pub fn tool_include_dir(&self, org: &str, name: &str, version: &str) -> PathBuf {
        self.cache_dir
            .join("components")
            .join("include")
            .join(org)
            .join(name)
            .join(version)
    }

    /// The cached `env` map for a tool version (TASKS/023). JSON object of
    /// string→string written on cold fetch, read on every run.
    pub fn tool_include_env_file(&self, org: &str, name: &str, version: &str) -> PathBuf {
        self.tool_include_dir(org, name, version).join("env.json")
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
        assert_eq!(
            fixed().user_config_cue(),
            PathBuf::from("/cfg/forest/forest.cue")
        );
    }

    #[test]
    fn lockfile_lives_under_state_dir() {
        // §1a.4 — XDG_STATE_HOME, not config, not cache.
        assert_eq!(
            fixed().lockfile(),
            PathBuf::from("/state/forest/forest.lock")
        );
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
    fn cached_binary_dir_strips_sha256_prefix() {
        assert_eq!(
            fixed().cached_binary_dir("sha256:abc123"),
            PathBuf::from("/cache/forest/components/bin/abc123"),
        );
        assert_eq!(
            fixed().cached_binary_dir("abc123"),
            PathBuf::from("/cache/forest/components/bin/abc123"),
        );
    }

    #[test]
    fn cached_binary_nests_the_real_name_under_the_hash_dir() {
        // DATA-510: the hash is a directory; the file inside carries the
        // component name so argv[0] reads as the tool, not the digest.
        assert_eq!(
            fixed().cached_binary("sha256:abc123", "shiitake"),
            PathBuf::from("/cache/forest/components/bin/abc123/shiitake"),
        );
        assert_eq!(
            fixed().cached_binary("abc123", "gitnow"),
            PathBuf::from("/cache/forest/components/bin/abc123/gitnow"),
        );
    }

    #[test]
    fn cached_binary_file_name_is_exactly_the_bin_name() {
        // The property `forest global run` relies on: basename(exec path) is
        // what the child sees as argv[0].
        let p = fixed().cached_binary("abc123", "forest-hello");
        assert_eq!(p.file_name().unwrap(), "forest-hello");
    }

    #[test]
    fn two_names_for_one_hash_share_a_directory() {
        let a = fixed().cached_binary("abc123", "rg");
        let b = fixed().cached_binary("abc123", "ripgrep");
        assert_eq!(a.parent(), b.parent());
        assert_eq!(a.parent().unwrap(), fixed().cached_binary_dir("abc123"));
    }

    #[test]
    fn legacy_binary_file_is_the_path_the_hash_dir_now_occupies() {
        // Migration hinges on these being the same path: what used to be a
        // file becomes a directory.
        assert_eq!(
            fixed().legacy_cached_binary_file("sha256:abc123"),
            fixed().cached_binary_dir("abc123"),
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
