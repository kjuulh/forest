//! Local build steps: cross-compile guest (musl) + runner (host gnu), and
//! pack the rootfs into an ext4 image using docker + `mkfs.ext4 -d`.
//!
//! Caching is mtime-based: we skip a step if the output is newer than every
//! input we know about. Good enough for `cargo test` — `cargo clean` invalidates.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::config::Config;

const GUEST_TARGET: &str = "x86_64-unknown-linux-musl";
const RUNNER_TARGET: &str = "x86_64-unknown-linux-gnu";

pub struct BuildArtifacts {
    pub runner_bin: PathBuf,
    pub agent_bin: PathBuf,
    pub rootfs_ext4: PathBuf,
}

pub fn build(cfg: &Config) -> anyhow::Result<BuildArtifacts> {
    std::fs::create_dir_all(&cfg.local_target_dir).context("create local_target_dir")?;

    // Guest binary is consumed by the rootfs build; not shipped separately.
    let guest_bin = build_cargo(cfg, "hollow-guest", GUEST_TARGET, true)?;
    let runner_bin = build_cargo(cfg, "hollow-test-runner", RUNNER_TARGET, false)?;
    // hollow-agent ships to the remote KVM host (same target as the runner).
    let agent_bin = build_cargo(cfg, "hollow-agent", RUNNER_TARGET, false)?;
    // hollow-controller runs locally on the dev machine during orchestrator
    // tests — built here so the path `target/release/hollow-controller` is
    // populated before orchestrator::start tries to spawn it, but we don't
    // need to keep the path around since it's derivable from cfg.repo_root.
    build_cargo_host(cfg, "hollow-controller")?;
    let rootfs_ext4 = build_base_ext4(cfg, &guest_bin)?;

    Ok(BuildArtifacts {
        runner_bin,
        agent_bin,
        rootfs_ext4,
    })
}

fn build_cargo_host(cfg: &Config, pkg: &str) -> anyhow::Result<PathBuf> {
    tracing::info!(pkg, "cargo build (release, host target)");
    let status = Command::new("cargo")
        .current_dir(&cfg.repo_root)
        .args(["build", "-p", pkg, "--release"])
        .status()
        .with_context(|| format!("spawn cargo build for {pkg}"))?;
    if !status.success() {
        bail!("cargo build {pkg} (host) failed");
    }
    let bin_path = cfg.repo_root.join("target/release").join(pkg);
    if !bin_path.exists() {
        bail!("expected binary not found: {}", bin_path.display());
    }
    Ok(bin_path)
}

fn build_cargo(
    cfg: &Config,
    pkg: &str,
    target: &str,
    needs_target_install: bool,
) -> anyhow::Result<PathBuf> {
    if needs_target_install {
        ensure_rustup_target(target)?;
    }

    tracing::info!(pkg, target, "cargo build (release)");
    let status = Command::new("cargo")
        .current_dir(&cfg.repo_root)
        .args(["build", "-p", pkg, "--release", "--target", target])
        .status()
        .with_context(|| format!("spawn cargo build for {pkg}"))?;
    if !status.success() {
        bail!("cargo build {pkg} ({target}) failed");
    }

    let bin_path = cfg
        .repo_root
        .join("target")
        .join(target)
        .join("release")
        .join(pkg);
    if !bin_path.exists() {
        bail!("expected binary not found: {}", bin_path.display());
    }
    Ok(bin_path)
}

fn ensure_rustup_target(target: &str) -> anyhow::Result<()> {
    let installed = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .context("rustup target list --installed")?;
    if !installed.status.success() {
        bail!("rustup target list failed: {:?}", installed.status);
    }
    let stdout = String::from_utf8_lossy(&installed.stdout);
    if stdout.lines().any(|l| l.trim() == target) {
        return Ok(());
    }

    tracing::info!(target, "installing rustup target");
    let status = Command::new("rustup")
        .args(["target", "add", target])
        .status()
        .context("rustup target add")?;
    if !status.success() {
        bail!("rustup target add {target} failed");
    }
    Ok(())
}

fn build_base_ext4(cfg: &Config, guest_bin: &Path) -> anyhow::Result<PathBuf> {
    let images_dir = cfg.repo_root.join("images");
    let dockerfile = images_dir.join("Dockerfile.base");
    if !dockerfile.exists() {
        bail!("missing {}", dockerfile.display());
    }

    let out = cfg.local_target_dir.join("base.ext4");
    if out.exists() && newer_than(&out, guest_bin)? && newer_than(&out, &dockerfile)? {
        tracing::info!(path = %out.display(), "base.ext4 up-to-date, reusing");
        return Ok(out);
    }

    let staged_guest = images_dir.join("hollow-guest");
    std::fs::copy(guest_bin, &staged_guest).with_context(|| {
        format!(
            "stage guest binary {} → {}",
            guest_bin.display(),
            staged_guest.display()
        )
    })?;
    // Dockerfile.base does `COPY hollow-guest …`, so it must be readable by docker.

    let image_tag = "hollow-base:test";
    tracing::info!(tag = image_tag, "docker build base image");
    let status = Command::new("docker")
        .args([
            "build",
            "-t",
            image_tag,
            "-f",
            dockerfile.to_string_lossy().as_ref(),
            images_dir.to_string_lossy().as_ref(),
        ])
        .status()
        .context("docker build")?;
    let _ = std::fs::remove_file(&staged_guest);
    if !status.success() {
        bail!("docker build base image failed");
    }

    // Export the image rootfs into a staging directory.
    let staging = cfg.local_target_dir.join("rootfs-staging");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).context("create staging")?;

    let cid_out = Command::new("docker")
        .args(["create", image_tag])
        .output()
        .context("docker create")?;
    if !cid_out.status.success() {
        bail!(
            "docker create failed: {}",
            String::from_utf8_lossy(&cid_out.stderr)
        );
    }
    let cid = String::from_utf8(cid_out.stdout)?.trim().to_string();

    let export_status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "docker export {cid} | tar -xf - -C {}",
            staging.to_string_lossy()
        ))
        .status()
        .context("docker export | tar -x")?;
    let _ = Command::new("docker").args(["rm", &cid]).status();
    if !export_status.success() {
        bail!("docker export failed");
    }

    // mkfs.ext4 -d <dir> populates the FS image without needing root/mount.
    if out.exists() {
        std::fs::remove_file(&out)?;
    }
    let status = Command::new("truncate")
        .args(["-s", "256M", out.to_string_lossy().as_ref()])
        .status()
        .context("truncate ext4 image")?;
    if !status.success() {
        bail!("truncate failed");
    }
    let status = Command::new("mkfs.ext4")
        .args([
            "-F",
            "-d",
            staging.to_string_lossy().as_ref(),
            "-L",
            "hollow-base",
            out.to_string_lossy().as_ref(),
        ])
        .status()
        .context("mkfs.ext4 -d")?;
    if !status.success() {
        bail!("mkfs.ext4 failed");
    }

    tracing::info!(path = %out.display(), "base.ext4 built");
    Ok(out)
}

fn newer_than(a: &Path, b: &Path) -> anyhow::Result<bool> {
    let am = std::fs::metadata(a)?.modified()?;
    let bm = std::fs::metadata(b)?.modified()?;
    Ok(am >= bm)
}
