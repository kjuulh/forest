//! `forest self update` / `forest self check` — keep the forest CLI
//! current. Because `forest` is the bootstrap tool for the whole
//! org workflow, there is no outer package manager to do this — the
//! binary has to update itself.
//!
//! Implementation matches `scripts/install.sh`: shell out to `gh`
//! (which the user already has authenticated for repo access) for
//! both the version check and the artifact download, then atomically
//! replace the running binary via `install(1)`.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Context;
use clap::{Parser, Subcommand};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::state::State;

/// GitHub repo the releases live on. Hard-coded because this is the
/// canonical distribution location — no benefit to making it
/// configurable.
const REPO: &str = "understory-io/forest";

/// How long a successful version check is considered fresh before we
/// hit the network again. 24h keeps the noise low while still
/// catching new releases reasonably quickly.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Parser)]
pub struct SelfCommand {
    #[command(subcommand)]
    sub: SelfSub,
}

#[derive(Subcommand)]
enum SelfSub {
    /// Replace this forest binary with the latest (or pinned) release.
    Update(UpdateArgs),
    /// Print whether a newer release exists. Exits 0 if up-to-date,
    /// 1 if a newer version is available, 2 on check failure.
    Check,
}

#[derive(Parser)]
struct UpdateArgs {
    /// Specific tag to install (e.g. `v0.2.0` or `0.2.0`). Defaults to
    /// the latest release.
    version: Option<String>,
}

impl SelfCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        match &self.sub {
            SelfSub::Update(args) => perform_update(args.version.as_deref()).await,
            SelfSub::Check => print_check_status().await,
        }
    }
}

// ─── Version probing ────────────────────────────────────────────────

fn current_version() -> Version {
    // CARGO_PKG_VERSION is set at compile time from the forest crate's
    // Cargo.toml. release-please owns that field on every release PR.
    Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is valid semver")
}

/// Strip the `v` prefix from a release tag and parse as semver.
fn tag_to_version(tag: &str) -> anyhow::Result<Version> {
    let stripped = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(stripped).with_context(|| format!("parse version from tag {tag:?}"))
}

/// Ask `gh` for the repo's "latest" release. Falls back gracefully if
/// `gh` is missing or unauthenticated — callers must treat this as
/// best-effort.
async fn fetch_latest_version() -> anyhow::Result<Version> {
    let output = tokio::process::Command::new("gh")
        .args([
            "release", "view",
            "--repo", REPO,
            "--json", "tagName",
            "--jq", ".tagName",
        ])
        .output()
        .await
        .context("invoke `gh release view` — is the GitHub CLI installed?")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("gh release view failed: {}", stderr.trim());
    }
    let tag = String::from_utf8_lossy(&output.stdout).trim().to_string();
    tag_to_version(&tag)
}

async fn print_check_status() -> anyhow::Result<()> {
    let current = current_version();
    match fetch_latest_version().await {
        Ok(latest) if latest > current => {
            println!("forest is outdated: current {current}, latest {latest}");
            println!("Run `forest self update` to upgrade.");
            std::process::exit(1);
        }
        Ok(latest) => {
            println!("forest is up to date ({current})");
            if latest < current {
                println!(
                    "(running a newer version than the latest release — local dev build?)"
                );
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("forest self check: version probe failed: {e}");
            std::process::exit(2);
        }
    }
}

// ─── Update mechanism ───────────────────────────────────────────────

/// Map the current host platform to the release target triple our
/// build matrix emits (see `.github/workflows/release.yml`).
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

async fn perform_update(version: Option<&str>) -> anyhow::Result<()> {
    let target_tag = match version {
        Some(v) if v.starts_with('v') => v.to_string(),
        Some(v) => format!("v{v}"),
        None => format!("v{}", fetch_latest_version().await?),
    };
    let target_triple = release_target()?;
    let asset = format!("forest-{target_tag}-{target_triple}.tar.gz");
    let checksum = format!("{asset}.sha256");

    let tmp = tempfile::tempdir().context("create temp dir for download")?;

    eprintln!("==> Downloading {asset}…");
    // `gh release download` will fail loudly if the tag doesn't exist
    // or the user lacks repo access. It also writes its own progress
    // output to stdout, so we don't need to wrap it in a spinner here.
    let status = tokio::process::Command::new("gh")
        .args([
            "release", "download", &target_tag,
            "--repo", REPO,
            "--pattern", &asset,
            "--pattern", &checksum,
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

    eprintln!("==> Installing to {}…", current_exe.display());
    replace_binary(&new_binary, &current_exe)?;

    eprintln!("==> forest {target_tag} installed");
    Ok(())
}

/// Verify the freshly-downloaded tarball against the matching
/// `*.sha256` file in `dir`. Picks whichever SHA-256 tool the host
/// provides:
///
///   - `sha256sum` (coreutils, default on Linux)
///   - `shasum -a 256` (BSD / macOS, ships with Perl)
///
/// Both consume the same `<hex>  <filename>` format, so either's
/// `-c` mode validates the file produced by either tool on the
/// build side.
async fn verify_sha256(dir: &Path, checksum_file: &str) -> anyhow::Result<()> {
    // Candidates in priority order: (tool name, args before the file).
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
            // Tool ran, verify either passed or failed loudly — done.
            Ok(status) if status.success() => return Ok(()),
            Ok(_) => {
                anyhow::bail!("checksum verification failed for {checksum_file}");
            }
            // Tool missing: silently try the next one.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("invoke {tool}")),
        }
    }
    anyhow::bail!(
        "neither `sha256sum` nor `shasum` is on PATH; cannot verify download. \
         Install coreutils (Linux) or ensure shasum is available (macOS) and retry."
    )
}

/// Atomically replace the current binary with the freshly-downloaded
/// one. Tries unprivileged `install(1)` first; falls back to `sudo`
/// if the destination needs root.
///
/// `install(1)` works on a running binary on Unix because the kernel
/// keeps the old inode alive for the current process — new
/// invocations pick up the new file.
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

    eprintln!("==> {} needs root; retrying with sudo", current_exe.display());
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

// ─── Background nag ─────────────────────────────────────────────────
//
// Called at the end of every successful forest command (see cli.rs).
// Cached so we only hit GitHub once per `CHECK_INTERVAL`. Silent on
// any failure — a broken version check must never break the user's
// actual command.

#[derive(Serialize, Deserialize, Default)]
struct UpdateCache {
    /// Seconds since UNIX epoch of the last successful probe.
    last_check_unix: u64,
    /// Tag name of the latest release at the time of probe, without
    /// the `v` prefix. Empty / missing means "probe failed".
    latest_version: Option<String>,
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
    let Ok(bytes) = serde_json::to_vec_pretty(cache) else { return };
    let _ = std::fs::write(&path, bytes);
}

/// Print a one-line nag if a newer release exists. Best-effort:
/// silent on any failure (no `gh`, no network, no cache directory).
///
/// Skip conditions:
///   - `CI=true` env (most CI systems set this)
///   - `FOREST_NO_UPDATE_CHECK` env set (any value) — explicit opt-out
///   - stderr isn't a TTY (piping forest output to a file, redirecting in tests)
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

    let cache = read_cache();
    let now = now_unix();
    let stale = cache
        .as_ref()
        .map(|c| now.saturating_sub(c.last_check_unix) >= CHECK_INTERVAL.as_secs())
        .unwrap_or(true);

    let latest_str: String = if stale {
        match fetch_latest_version().await {
            Ok(v) => {
                let s = v.to_string();
                write_cache(&UpdateCache {
                    last_check_unix: now,
                    latest_version: Some(s.clone()),
                });
                s
            }
            // Silent fail — `gh` might be missing, no auth, no network.
            // Don't pester users mid-command about it.
            Err(_) => return,
        }
    } else {
        match cache.and_then(|c| c.latest_version) {
            Some(v) => v,
            None => return,
        }
    };

    let Ok(latest) = Version::parse(&latest_str) else {
        return;
    };
    let current = current_version();
    if latest > current {
        // Single line, leading newline so it isn't glued to the
        // command's last bit of output.
        eprintln!(
            "\n✨ forest {latest} is available (you have {current}). \
             Run `forest self update` to upgrade."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_version_parses() {
        // Just confirms CARGO_PKG_VERSION is valid semver — the assert
        // is implicit in `current_version()` itself (it'd panic).
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
    fn release_target_compiles_for_current_platform() {
        // Smoke test: just ensure release_target returns *something*
        // (Ok or Err) without panicking on the current host.
        let _ = release_target();
    }

    #[test]
    fn update_cache_roundtrips_through_json() {
        let cache = UpdateCache {
            last_check_unix: 1_700_000_000,
            latest_version: Some("0.2.3".into()),
        };
        let json = serde_json::to_string(&cache).unwrap();
        let parsed: UpdateCache = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.last_check_unix, 1_700_000_000);
        assert_eq!(parsed.latest_version.as_deref(), Some("0.2.3"));
    }
}
