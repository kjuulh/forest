use std::path::PathBuf;

const APP_DIR: &str = "forest";
const CONFIG_FILE: &str = "forest.cue";
const LEGACY_CONFIG_FILE: &str = "forest.toml";
const LOCKFILE: &str = "forest.lock";
const LOCK_GUARD_FILE: &str = ".lock";

#[derive(Debug, Clone)]
pub struct GlobalPaths {
    config_dir: PathBuf,
    state_dir: PathBuf,
    cache_dir: PathBuf,
}

impl GlobalPaths {
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

    pub fn cached_binary_dir(&self, sha: &str) -> PathBuf {
        let hex = sha.strip_prefix("sha256:").unwrap_or(sha);
        self.binary_cache_dir().join(hex)
    }

    pub fn cached_binary(&self, sha: &str, bin_name: &str) -> PathBuf {
        self.cached_binary_dir(sha).join(bin_name)
    }

    pub fn legacy_cached_binary_file(&self, sha: &str) -> PathBuf {
        self.cached_binary_dir(sha)
    }

    pub fn tool_include_dir(&self, org: &str, name: &str, version: &str) -> PathBuf {
        self.cache_dir
            .join("components")
            .join("include")
            .join(org)
            .join(name)
            .join(version)
    }

    pub fn tool_include_env_file(&self, org: &str, name: &str, version: &str) -> PathBuf {
        self.tool_include_dir(org, name, version).join("env.json")
    }

    pub fn tool_include_shell_file(&self, org: &str, name: &str, version: &str) -> PathBuf {
        self.tool_include_dir(org, name, version).join("shell.json")
    }

    pub fn tool_shell_snippet(&self, org: &str, name: &str, version: &str, shell: &str) -> PathBuf {
        self.tool_include_dir(org, name, version)
            .join("shell")
            .join(format!("{shell}.sh"))
    }

    pub fn shell_aggregate(&self, shell: &str) -> PathBuf {
        self.cache_dir()
            .join("global")
            .join("shell")
            .join(format!("{shell}.sh"))
    }

    pub fn shell_aggregate_dir(&self) -> PathBuf {
        self.cache_dir().join("global").join("shell")
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
        assert_eq!(
            fixed().legacy_cached_binary_file("sha256:abc123"),
            fixed().cached_binary_dir("abc123"),
        );
    }

    #[test]
    fn shell_declaration_sits_beside_the_env_cache() {
        let p = fixed();
        assert_eq!(
            p.tool_include_shell_file("understory", "gitnow", "0.5.0"),
            PathBuf::from("/cache/forest/components/include/understory/gitnow/0.5.0/shell.json"),
        );
        assert_eq!(
            p.tool_include_shell_file("understory", "gitnow", "0.5.0")
                .parent(),
            p.tool_include_env_file("understory", "gitnow", "0.5.0")
                .parent(),
        );
    }

    #[test]
    fn snippets_are_keyed_by_version_and_shell() {
        let p = fixed();
        assert_eq!(
            p.tool_shell_snippet("understory", "gitnow", "0.5.0", "zsh"),
            PathBuf::from("/cache/forest/components/include/understory/gitnow/0.5.0/shell/zsh.sh"),
        );
        assert_ne!(
            p.tool_shell_snippet("understory", "gitnow", "0.5.0", "zsh"),
            p.tool_shell_snippet("understory", "gitnow", "0.5.1", "zsh"),
        );
        assert_ne!(
            p.tool_shell_snippet("understory", "gitnow", "0.5.0", "zsh"),
            p.tool_shell_snippet("understory", "gitnow", "0.5.0", "fish"),
        );
    }

    #[test]
    fn aggregate_is_one_stable_path_per_shell() {
        let p = fixed();
        assert_eq!(
            p.shell_aggregate("zsh"),
            PathBuf::from("/cache/forest/global/shell/zsh.sh"),
        );
        assert_eq!(
            p.shell_aggregate("zsh").parent().unwrap(),
            p.shell_aggregate_dir(),
        );
    }

    #[test]
    fn aggregate_lives_next_to_the_shims_not_inside_them() {
        let p = fixed();
        assert_ne!(p.shell_aggregate_dir(), p.shims_dir());
        assert!(!p.shell_aggregate("zsh").starts_with(p.shims_dir()));
    }

    #[test]
    fn write_lock_guard_lives_in_config_dir() {
        assert_eq!(
            fixed().write_lock_guard(),
            PathBuf::from("/cfg/forest/.lock"),
        );
    }
}
