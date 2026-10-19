use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Context;
use clap::{Parser, Subcommand};
use semver::Version;
use serde::{Deserialize, Serialize};

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

fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is valid semver")
}

fn tag_to_version(tag: &str) -> anyhow::Result<Version> {
    let stripped = tag.strip_prefix('v').unwrap_or(tag);
    Version::parse(stripped).with_context(|| format!("parse version from tag {tag:?}"))
}

async fn fetch_latest_version() -> anyhow::Result<Version> {
    let output = tokio::process::Command::new("gh")
        .args([
            "release", "view", "--repo", REPO, "--json", "tagName", "--jq", ".tagName",
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

    eprintln!("==> Installing to {}…", current_exe.display());
    replace_binary(&new_binary, &current_exe)?;

    eprintln!("==> forest {target_tag} installed");
    Ok(())
}

async fn verify_sha256(dir: &Path, checksum_file: &str) -> anyhow::Result<()> {
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
        eprintln!(
            "\n✨ forest {latest} is available (you have {current}). \
             Run `forest self update` to upgrade.\n   \
             Release notes: https://github.com/{REPO}/releases/tag/v{latest}"
        );
    }
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
    fn release_target_compiles_for_current_platform() {
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
