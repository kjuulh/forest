use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Context;
use clap::{Parser, Subcommand};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::global::service::{PRERELEASE_ENV, prerelease_default_from};
use crate::state::State;

const REPO: &str = "understory-io/forest";

const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Parser)]
pub struct SelfCommand {
    #[command(subcommand)]
    sub: SelfSub,
}

#[derive(Subcommand)]
enum SelfSub {
    /// Replace this forest binary with the latest stable release, or an exact one.
    Update(UpdateArgs),
    /// Print whether a newer release exists. Exits 0 if up-to-date,
    /// 1 if a newer version is available, 2 on check failure.
    Check(CheckArgs),
}

#[derive(Parser)]
struct UpdateArgs {
    /// Exact release to install (e.g. `v0.3.16-rc.1` or `0.3.15`), older
    /// ones included. Defaults to the latest stable release, and never
    /// replaces this binary with an older one unless named here.
    #[arg(conflicts_with = "prerelease")]
    version: Option<String>,
    /// Consider prereleases too: install the highest SemVer release, stable
    /// or prerelease, for this run. `FOREST_PRERELEASE=1` does the same for
    /// every run while it is set, as it does for global tools.
    #[arg(long)]
    prerelease: bool,
}

#[derive(Parser)]
struct CheckArgs {
    /// Compare against the highest release including prereleases, as
    /// `FOREST_PRERELEASE=1` does while it is set.
    #[arg(long)]
    prerelease: bool,
}

impl SelfCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        match &self.sub {
            SelfSub::Update(args) => {
                perform_update(
                    args.version.as_deref(),
                    args.prerelease || follows_prereleases(),
                )
                .await
            }
            SelfSub::Check(args) => {
                print_check_status(args.prerelease || follows_prereleases()).await
            }
        }
    }
}

fn follows_prereleases() -> bool {
    prerelease_default_from(std::env::var_os(PRERELEASE_ENV))
}

pub fn long_version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    match option_env!("FOREST_BUILD_COMMIT") {
        Some(commit) if !commit.is_empty() => format!("{version} (commit {commit})"),
        _ => format!("{version} (local build)"),
    }
}

fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is valid semver")
}

fn tag_to_version(tag: &str) -> anyhow::Result<Version> {
    let stripped = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(stripped).with_context(|| {
        format!("{tag:?} is not a release version; expected SemVer such as v0.3.16 or v0.3.16-rc.1")
    })
}

fn reported_version(version_output: &str) -> Option<Version> {
    let mut words = version_output.split_whitespace();
    if words.next()? != "forest" {
        return None;
    }
    Version::parse(words.next()?).ok()
}

async fn gh_output(args: &[&str]) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("gh")
        .args(args)
        .output()
        .await
        .context("invoke `gh` — is the GitHub CLI installed?")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("gh {} failed: {}", args[..2].join(" "), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn fetch_latest_version() -> anyhow::Result<Version> {
    let tag = gh_output(&[
        "release", "view", "--repo", REPO, "--json", "tagName", "--jq", ".tagName",
    ])
    .await?;
    let version = tag_to_version(&tag)?;
    if !version.pre.is_empty() {
        anyhow::bail!(
            "the latest stable release {tag} carries a prerelease version; it was published \
             without GitHub's prerelease flag, so it is not taken as stable. Install it \
             explicitly with `forest self update {tag}` if that is what you want."
        );
    }
    Ok(version)
}

#[derive(Deserialize)]
struct ReleaseEntry {
    tag_name: String,
    #[serde(rename = "prerelease")]
    is_prerelease: bool,
    #[serde(rename = "draft")]
    is_draft: bool,
}

fn ranked_releases(entries: &[ReleaseEntry], include_prereleases: bool) -> Vec<(Version, String)> {
    let mut ranked: Vec<(Version, String)> = entries
        .iter()
        .filter(|e| !e.is_draft)
        .filter_map(|e| {
            let version = tag_to_version(&e.tag_name).ok()?;
            let semver_prerelease = !version.pre.is_empty();
            if semver_prerelease != e.is_prerelease {
                return None;
            }
            if semver_prerelease && !include_prereleases {
                return None;
            }
            Some((version, e.tag_name.clone()))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0));
    ranked
}

async fn fetch_ranked_releases(
    include_prereleases: bool,
) -> anyhow::Result<Vec<(Version, String)>> {
    let json = gh_output(&[
        "api",
        "--paginate",
        "--slurp",
        "repos/understory-io/forest/releases?per_page=100",
    ])
    .await?;
    let pages: Vec<Vec<ReleaseEntry>> =
        serde_json::from_str(&json).context("parse paginated GitHub releases output")?;
    let entries: Vec<ReleaseEntry> = pages.into_iter().flatten().collect();
    Ok(ranked_releases(&entries, include_prereleases))
}

#[derive(Deserialize)]
struct ReleaseAssets {
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
}

async fn release_has_assets(tag: &str, wanted: &[&str]) -> anyhow::Result<bool> {
    let json = gh_output(&["release", "view", tag, "--repo", REPO, "--json", "assets"]).await?;
    let release: ReleaseAssets =
        serde_json::from_str(&json).context("parse `gh release view` output")?;
    Ok(wanted
        .iter()
        .all(|w| release.assets.iter().any(|a| a.name == *w)))
}

async fn highest_installable_release(target_triple: &str) -> anyhow::Result<Version> {
    let ranked = fetch_ranked_releases(true).await?;
    for (version, tag) in &ranked {
        let asset = asset_name(tag, target_triple);
        let checksum = format!("{asset}.sha256");
        if release_has_assets(tag, &[&asset, &checksum]).await? {
            return Ok(version.clone());
        }
        eprintln!("==> {tag} has no {asset} yet (still building?) — looking further back");
    }
    anyhow::bail!("no release carries {target_triple} assets")
}

async fn print_check_status(prerelease: bool) -> anyhow::Result<()> {
    let current = current_version();
    let latest = if prerelease {
        fetch_ranked_releases(true).await.and_then(|r| {
            r.into_iter()
                .next()
                .map(|(v, _)| v)
                .context("no releases found")
        })
    } else {
        fetch_latest_version().await
    };
    let update_cmd = if prerelease && !follows_prereleases() {
        "forest self update --prerelease"
    } else {
        "forest self update"
    };
    match latest {
        Ok(latest) if latest > current => {
            println!("forest is outdated: current {current}, latest {latest}");
            println!("Run `{update_cmd}` to upgrade.");
            std::process::exit(1);
        }
        Ok(latest) => {
            println!("forest is up to date ({current})");
            if latest < current && !current.pre.is_empty() && !prerelease {
                println!(
                    "(running prerelease {current}; the latest stable release is {latest}. \
                     `forest self update v{latest}` returns to it.)"
                );
            } else if latest < current {
                println!("(running a newer version than the latest release — local dev build?)");
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("forest self check: version probe failed: {e}");
            std::process::exit(2);
        }
    }
}

fn release_target() -> anyhow::Result<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok("x86_64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Ok("aarch64-unknown-linux-gnu")
    } else {
        anyhow::bail!(
            "self-update is not supported on this platform (forest ships for \
             macOS aarch64 and Linux x86_64/aarch64). Install manually."
        )
    }
}

fn asset_name(tag: &str, target_triple: &str) -> String {
    format!("forest-{tag}-{target_triple}.tar.gz")
}

async fn perform_update(version: Option<&str>, prerelease: bool) -> anyhow::Result<()> {
    let target_triple = release_target()?;
    let current = current_version();
    let target = match version {
        Some(v) => tag_to_version(v)?,
        None => {
            let candidate = if prerelease {
                highest_installable_release(target_triple).await?
            } else {
                fetch_latest_version().await?
            };
            if candidate < current {
                let kind = if prerelease {
                    "release"
                } else {
                    "stable release"
                };
                eprintln!(
                    "==> forest {current} is newer than the latest {kind} {candidate}; leaving it in place."
                );
                eprintln!("    To install {candidate} anyway: forest self update v{candidate}");
                return Ok(());
            }
            candidate
        }
    };
    let target_tag = format!("v{target}");
    let asset = asset_name(&target_tag, target_triple);
    let checksum = format!("{asset}.sha256");

    let tmp = tempfile::tempdir().context("create temp dir for download")?;

    eprintln!("==> Downloading {asset}…");
    let status = tokio::process::Command::new("gh")
        .args([
            "release",
            "download",
            &target_tag,
            "--repo",
            REPO,
            "--pattern",
            &asset,
            "--pattern",
            &checksum,
            "--dir",
        ])
        .arg(tmp.path())
        .status()
        .await
        .context("invoke `gh release download`")?;
    if !status.success() {
        anyhow::bail!(
            "gh release download failed for {target_tag}/{asset} — \
             check the tag exists and you have repo access (`gh auth status`)"
        );
    }

    eprintln!("==> Verifying checksum…");
    verify_sha256(tmp.path(), &checksum).await?;

    eprintln!("==> Extracting…");
    let status = tokio::process::Command::new("tar")
        .args(["-xzf", &asset])
        .current_dir(tmp.path())
        .status()
        .await
        .context("invoke tar")?;
    if !status.success() {
        anyhow::bail!("tar extraction failed");
    }

    let current_exe = std::env::current_exe().context("locate current forest binary")?;
    let new_binary = tmp.path().join("forest");

    let identity = verify_identity(&new_binary, &target).await?;

    eprintln!("==> Installing to {}…", current_exe.display());
    replace_binary(&new_binary, &current_exe)?;

    eprintln!("==> {identity} installed");
    Ok(())
}

async fn verify_identity(binary: &Path, expected: &Version) -> anyhow::Result<String> {
    let output = tokio::process::Command::new(binary)
        .arg("--version")
        .output()
        .await
        .with_context(|| format!("run {} --version", binary.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    match reported_version(&stdout) {
        Some(reported) if reported == *expected && output.status.success() => Ok(stdout),
        _ => anyhow::bail!(
            "refusing to install: the downloaded binary reports {stdout:?}, not forest {expected}"
        ),
    }
}

async fn verify_sha256(dir: &Path, checksum_file: &str) -> anyhow::Result<()> {
    let checksum = tokio::fs::read_to_string(dir.join(checksum_file))
        .await
        .context("read archive checksum")?;
    let asset = checksum_file
        .strip_suffix(".sha256")
        .context("invalid checksum filename")?;
    validate_checksum_record(&checksum, asset)?;
    let candidates: &[(&str, &[&str])] =
        &[("sha256sum", &["-c"]), ("shasum", &["-a", "256", "-c"])];

    for (tool, args) in candidates {
        let result = tokio::process::Command::new(tool)
            .args(*args)
            .arg(checksum_file)
            .current_dir(dir)
            .status()
            .await;
        match result {
            Ok(status) if status.success() => return Ok(()),
            Ok(_) => {
                anyhow::bail!("checksum verification failed for {checksum_file}");
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("invoke {tool}")),
        }
    }
    anyhow::bail!(
        "neither `sha256sum` nor `shasum` is on PATH; cannot verify download. \
         Install coreutils (Linux) or ensure shasum is available (macOS) and retry."
    )
}

fn validate_checksum_record(checksum: &str, asset: &str) -> anyhow::Result<()> {
    let mut fields = checksum.split_whitespace();
    let digest = fields.next().context("empty archive checksum")?;
    let filename = fields.next().context("missing archive checksum filename")?;
    if digest.len() != 64
        || !digest.bytes().all(|b| b.is_ascii_hexdigit())
        || filename.strip_prefix('*').unwrap_or(filename) != asset
        || fields.next().is_some()
    {
        anyhow::bail!("checksum must contain exactly one SHA-256 record for {asset}");
    }
    Ok(())
}

fn replace_binary(new_binary: &Path, current_exe: &Path) -> anyhow::Result<()> {
    let direct = std::process::Command::new("install")
        .args(["-m", "0755"])
        .arg(new_binary)
        .arg(current_exe)
        .status();

    match direct {
        Ok(s) if s.success() => return Ok(()),
        _ => {}
    }

    eprintln!(
        "==> {} needs root; retrying with sudo",
        current_exe.display()
    );
    let sudo = std::process::Command::new("sudo")
        .args(["install", "-m", "0755"])
        .arg(new_binary)
        .arg(current_exe)
        .status()
        .context("invoke sudo install")?;
    if !sudo.success() {
        anyhow::bail!(
            "sudo install failed — install manually with:\n  install -m 0755 {} {}",
            new_binary.display(),
            current_exe.display()
        );
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Default)]
struct UpdateCache {
    last_check_unix: u64,
    latest_version: Option<String>,
    #[serde(default)]
    prerelease: bool,
}

fn cache_path() -> Option<PathBuf> {
    let dir = dirs::cache_dir()?.join("forest");
    Some(dir.join("update-check.json"))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_cache() -> Option<UpdateCache> {
    let path = cache_path()?;
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_cache(cache: &UpdateCache) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(bytes) = serde_json::to_vec_pretty(cache) else {
        return;
    };
    let _ = std::fs::write(&path, bytes);
}

pub async fn maybe_print_update_nag() {
    if std::env::var_os("CI").is_some() {
        return;
    }
    if std::env::var_os("FOREST_NO_UPDATE_CHECK").is_some() {
        return;
    }
    if !std::io::stderr().is_terminal() {
        return;
    }

    let prerelease = follows_prereleases();
    let cache = read_cache();
    let now = now_unix();

    let latest_str: String = match cached_latest(cache.as_ref(), now, prerelease) {
        CachedLatest::Fresh(Some(v)) => v.to_string(),
        CachedLatest::Fresh(None) => return,
        CachedLatest::Stale => {
            let latest = if prerelease {
                fetch_ranked_releases(true)
                    .await
                    .ok()
                    .and_then(|r| r.into_iter().next().map(|(v, _)| v))
            } else {
                fetch_latest_version().await.ok()
            };
            match latest {
                Some(v) => {
                    let s = v.to_string();
                    write_cache(&UpdateCache {
                        last_check_unix: now,
                        latest_version: Some(s.clone()),
                        prerelease,
                    });
                    s
                }
                None => return,
            }
        }
    };

    let exe = std::env::current_exe().ok();
    if let Some(notice) = update_notice(&latest_str, &current_version(), exe.as_deref()) {
        eprintln!("{notice}");
    }
}

enum CachedLatest<'a> {
    Stale,
    Fresh(Option<&'a str>),
}

fn cached_latest(cache: Option<&UpdateCache>, now: u64, prerelease: bool) -> CachedLatest<'_> {
    match cache {
        Some(c)
            if c.prerelease == prerelease
                && now.saturating_sub(c.last_check_unix) < CHECK_INTERVAL.as_secs() =>
        {
            CachedLatest::Fresh(c.latest_version.as_deref())
        }
        _ => CachedLatest::Stale,
    }
}

fn update_notice(latest: &str, current: &Version, exe: Option<&Path>) -> Option<String> {
    let latest = Version::parse(latest).ok()?;
    if latest <= *current {
        return None;
    }
    let you_have = match exe {
        Some(exe) => format!("{current} at {}", exe.display()),
        None => current.to_string(),
    };
    Some(format!(
        "\n✨ forest {latest} is available (you have {you_have}). \
         Run `forest self update` to upgrade.\n   \
         Release notes: https://github.com/{REPO}/releases/tag/v{latest}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_version_parses() {
        let _ = current_version();
    }

    #[test]
    fn tag_to_version_strips_v_prefix() {
        assert_eq!(tag_to_version("v0.2.0").unwrap(), Version::new(0, 2, 0));
        assert_eq!(tag_to_version("0.2.0").unwrap(), Version::new(0, 2, 0));
    }

    #[test]
    fn tag_to_version_rejects_garbage() {
        assert!(tag_to_version("not-a-version").is_err());
        assert!(tag_to_version("vbad").is_err());
    }

    #[test]
    fn a_channel_name_is_not_a_release_version() {
        assert!(tag_to_version("unstable").is_err());
        assert!(tag_to_version("latest").is_err());
        assert_eq!(
            tag_to_version("v0.3.16-rc.1").unwrap(),
            Version::parse("0.3.16-rc.1").unwrap()
        );
    }

    fn release(tag: &str, is_prerelease: bool) -> ReleaseEntry {
        ReleaseEntry {
            tag_name: tag.into(),
            is_prerelease,
            is_draft: false,
        }
    }

    fn tags(ranked: &[(Version, String)]) -> Vec<&str> {
        ranked.iter().map(|(_, t)| t.as_str()).collect()
    }

    #[test]
    fn opting_in_ranks_prereleases_by_semver_not_by_text_or_publish_order() {
        let entries = [
            release("v0.3.16-alpha.10", true),
            release("v0.3.15", false),
            release("v0.3.16-rc.1", true),
            release("v0.3.16-alpha.2", true),
        ];
        assert_eq!(
            tags(&ranked_releases(&entries, true)),
            [
                "v0.3.16-rc.1",
                "v0.3.16-alpha.10",
                "v0.3.16-alpha.2",
                "v0.3.15"
            ]
        );
    }

    #[test]
    fn a_final_release_outranks_its_own_release_candidates() {
        let entries = [
            release("v0.3.16-rc.1", true),
            release("v0.3.16", false),
            release("v0.3.16-rc.2", true),
        ];
        assert_eq!(tags(&ranked_releases(&entries, true))[0], "v0.3.16");
    }

    #[test]
    fn without_opting_in_prereleases_are_never_candidates() {
        let entries = [release("v0.3.16-rc.1", true), release("v0.3.15", false)];
        assert_eq!(tags(&ranked_releases(&entries, false)), ["v0.3.15"]);
    }

    #[test]
    fn a_release_whose_prerelease_flag_disagrees_with_its_version_is_never_a_candidate() {
        let entries = [
            release("v0.3.16-rc.1", false),
            release("v0.3.17", true),
            release("v0.3.15", false),
        ];
        assert_eq!(tags(&ranked_releases(&entries, true)), ["v0.3.15"]);
        assert_eq!(tags(&ranked_releases(&entries, false)), ["v0.3.15"]);
    }

    #[test]
    fn drafts_and_non_semver_tags_are_never_candidates() {
        let entries = [
            ReleaseEntry {
                tag_name: "v0.4.0".into(),
                is_prerelease: false,
                is_draft: true,
            },
            release("nightly", true),
            release("v0.3.15", false),
        ];
        assert_eq!(tags(&ranked_releases(&entries, true)), ["v0.3.15"]);
    }

    #[test]
    fn identity_is_read_from_both_old_and_provenance_carrying_version_output() {
        assert_eq!(
            reported_version("forest 0.3.14\n"),
            Some(Version::new(0, 3, 14))
        );
        assert_eq!(
            reported_version("forest 0.3.16-rc.1 (commit 05b55e8)"),
            Some(Version::parse("0.3.16-rc.1").unwrap())
        );
        assert_eq!(reported_version("gitnow 0.3.16"), None);
        assert_eq!(reported_version(""), None);
    }

    #[test]
    fn a_checksum_for_another_file_cannot_authorize_the_archive() {
        let digest = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert!(
            validate_checksum_record(&format!("{digest}  /dev/null\n"), "forest.tar.gz").is_err()
        );
        assert!(
            validate_checksum_record(
                &format!("{digest}  forest.tar.gz\n{digest}  /dev/null\n"),
                "forest.tar.gz"
            )
            .is_err()
        );
        assert!(
            validate_checksum_record(&format!("{digest}  forest.tar.gz\n"), "forest.tar.gz")
                .is_ok()
        );
        assert!(
            validate_checksum_record(&format!("{digest} *forest.tar.gz\n"), "forest.tar.gz")
                .is_ok()
        );
    }

    #[test]
    fn release_target_compiles_for_current_platform() {
        let _ = release_target();
    }

    #[test]
    fn an_update_cache_from_an_older_forest_reads_as_a_stable_check() {
        let parsed: UpdateCache =
            serde_json::from_str(r#"{"last_check_unix": 1700000000, "latest_version": "0.2.3"}"#)
                .unwrap();
        assert_eq!(parsed.last_check_unix, 1_700_000_000);
        assert_eq!(parsed.latest_version.as_deref(), Some("0.2.3"));
        assert!(!parsed.prerelease);
    }

    fn cache(last_check_unix: u64, latest: &str, prerelease: bool) -> UpdateCache {
        UpdateCache {
            last_check_unix,
            latest_version: Some(latest.into()),
            prerelease,
        }
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn a_cache_older_than_a_day_or_from_the_other_channel_is_refetched() {
        let now = 1_800_000_000;
        let day = CHECK_INTERVAL.as_secs();
        assert!(matches!(
            cached_latest(None, now, false),
            CachedLatest::Stale
        ));
        assert!(matches!(
            cached_latest(Some(&cache(now - day, "0.3.17", false)), now, false),
            CachedLatest::Stale
        ));
        assert!(matches!(
            cached_latest(Some(&cache(now - 60, "0.3.17", false)), now, true),
            CachedLatest::Stale
        ));
        assert!(matches!(
            cached_latest(Some(&cache(now - day + 1, "0.3.17", false)), now, false),
            CachedLatest::Fresh(Some("0.3.17"))
        ));
    }

    #[test]
    fn a_replaced_binary_reading_the_same_cache_reports_its_own_version() {
        let cached = cache(1_800_000_000, "0.3.17", false);
        let CachedLatest::Fresh(Some(latest)) = cached_latest(Some(&cached), 1_800_000_060, false)
        else {
            panic!("a minute-old cache is fresh");
        };
        assert_eq!(update_notice(latest, &v("0.3.17"), None), None);
        let old = Path::new("/Users/me/.cargo/bin/forest");
        let notice = update_notice(latest, &v("0.3.7"), Some(old)).unwrap();
        assert!(
            notice.contains(
                "forest 0.3.17 is available (you have 0.3.7 at /Users/me/.cargo/bin/forest)"
            ),
            "{notice}"
        );
        assert!(notice.contains("/releases/tag/v0.3.17"), "{notice}");
    }

    #[test]
    fn the_notice_compares_versions_by_semver_not_by_text() {
        assert!(update_notice("0.3.17", &v("0.3.7"), None).is_some());
        assert!(update_notice("0.3.7", &v("0.3.17"), None).is_none());
        assert!(update_notice("0.3.17", &v("0.3.17"), None).is_none());
        assert!(update_notice("0.3.18-rc.1", &v("0.3.17"), None).is_some());
        assert!(update_notice("0.3.17", &v("0.3.18-rc.1"), None).is_none());
        assert!(update_notice("0.3.17", &v("0.3.17-rc.2"), None).is_some());
        assert!(update_notice("not-a-version", &v("0.3.7"), None).is_none());
    }
}
