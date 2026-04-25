//! Local build steps: cross-compile guest (musl) + runner/agent (host gnu),
//! then build each rootfs image (base, terraform-v1, …) as a docker image and
//! pack to ext4 via `mkfs.ext4 -d`.
//!
//! Caching is mtime-based: we skip a step if the output is newer than every
//! input we know about. Good enough for `cargo test` — `cargo clean` invalidates.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::config::Config;

const GUEST_TARGET: &str = "x86_64-unknown-linux-musl";
const RUNNER_TARGET: &str = "x86_64-unknown-linux-gnu";

/// Declarative description of a rootfs image we know how to build. The base
/// must come first — everything else derives from it.
struct ImageBuild {
    /// Logical name. Maps to `<name>.ext4` on the remote.
    name: &'static str,
    /// Dockerfile filename inside `hollow/images/`.
    dockerfile: &'static str,
    /// Docker image tag to use (and reference from downstream Dockerfiles).
    tag: &'static str,
    /// Filesystem label baked into the ext4.
    fs_label: &'static str,
    /// Size of the ext4 image. Override for anything bigger than base.
    size: &'static str,
}

const IMAGES: &[ImageBuild] = &[
    ImageBuild {
        name: "base",
        dockerfile: "Dockerfile.base",
        tag: "hollow-base:test",
        fs_label: "hollow-base",
        size: "256M",
    },
    ImageBuild {
        // Forest's destination registry calls this "terraform" (the binary
        // inside is actually OpenTofu — see Dockerfile for the rationale).
        name: "terraform-v1",
        dockerfile: "Dockerfile.terraform-v1",
        tag: "hollow-terraform-v1:test",
        fs_label: "hollow-tf",
        size: "1024M",
    },
    ImageBuild {
        // Forest's destination type is `forest/fluxv1/1` — controller maps
        // this to image `fluxv1-v1.ext4`. Ships git + openssh-client + flux
        // CLI + kustomize CLI; the real flux git-clone/push workflow comes
        // in a follow-up dispatcher arm.
        name: "fluxv1-v1",
        dockerfile: "Dockerfile.fluxv1",
        tag: "hollow-fluxv1-v1:test",
        fs_label: "hollow-flux",
        size: "1024M",
    },
];

pub struct BuildArtifacts {
    pub runner_bin: PathBuf,
    pub agent_bin: PathBuf,
    /// All rootfs images, keyed by logical name. The first entry is always
    /// `base.ext4` and the rest derive from it.
    pub images: Vec<ImageArtifact>,
}

#[derive(Debug, Clone)]
pub struct ImageArtifact {
    pub name: String,
    pub ext4_path: PathBuf,
}

pub fn build(cfg: &Config) -> anyhow::Result<BuildArtifacts> {
    std::fs::create_dir_all(&cfg.local_target_dir).context("create local_target_dir")?;

    let guest_bin = build_cargo(cfg, "hollow-guest", GUEST_TARGET, true)?;
    let runner_bin = build_cargo(cfg, "hollow-test-runner", RUNNER_TARGET, false)?;
    let agent_bin = build_cargo(cfg, "hollow-agent", RUNNER_TARGET, false)?;
    // Controller runs on the dev machine in orchestrator tests; build it so
    // target/release/hollow-controller is populated.
    build_cargo_host(cfg, "hollow-controller")?;

    let mut images = Vec::with_capacity(IMAGES.len());
    for spec in IMAGES {
        let art = build_image(cfg, spec, &guest_bin)?;
        images.push(art);
    }

    Ok(BuildArtifacts {
        runner_bin,
        agent_bin,
        images,
    })
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

fn build_image(cfg: &Config, spec: &ImageBuild, guest_bin: &Path) -> anyhow::Result<ImageArtifact> {
    let images_dir = cfg.repo_root.join("images");
    let dockerfile = images_dir.join(spec.dockerfile);
    if !dockerfile.exists() {
        bail!("missing {}", dockerfile.display());
    }

    let out = cfg.local_target_dir.join(format!("{}.ext4", spec.name));

    // Cache invalidation: only the base image depends on the guest binary
    // directly. Higher-layer images depend on whatever `FROM` they reference,
    // and docker's own layer cache handles that. Still re-pack whenever the
    // dockerfile changes.
    if out.exists() && newer_than(&out, &dockerfile)? && newer_than(&out, guest_bin)? {
        // Ensure the sidecar exists even on a cache hit — older harness runs
        // didn't write it, and bootstrap.rs needs it to ship.
        let sidecar = out.with_file_name(format!("{}.ext4.sha256", spec.name));
        if !sidecar.exists() {
            let digest = sha256_file(&out)
                .with_context(|| format!("hash {}", out.display()))?;
            std::fs::write(&sidecar, format!("{digest}\n"))
                .with_context(|| format!("write {}", sidecar.display()))?;
        }
        tracing::info!(image = spec.name, path = %out.display(), "up-to-date, reusing");
        return Ok(ImageArtifact {
            name: spec.name.to_string(),
            ext4_path: out,
        });
    }

    // Stage the guest binary into images/ for the Dockerfile.base COPY line.
    // Safe to stage for non-base images too — docker just ignores it.
    let staged_guest = images_dir.join("hollow-guest");
    std::fs::copy(guest_bin, &staged_guest).with_context(|| {
        format!(
            "stage guest binary {} → {}",
            guest_bin.display(),
            staged_guest.display()
        )
    })?;

    tracing::info!(image = spec.name, tag = spec.tag, "docker build");
    let status = Command::new("docker")
        .args([
            "build",
            "-t",
            spec.tag,
            "-f",
            dockerfile.to_string_lossy().as_ref(),
            images_dir.to_string_lossy().as_ref(),
        ])
        .status()
        .context("docker build")?;
    let _ = std::fs::remove_file(&staged_guest);
    if !status.success() {
        bail!("docker build {} failed", spec.name);
    }

    // Export the image rootfs into a staging directory.
    let staging = cfg
        .local_target_dir
        .join(format!("rootfs-staging-{}", spec.name));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).context("create staging")?;

    let cid_out = Command::new("docker")
        .args(["create", spec.tag])
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

    if out.exists() {
        std::fs::remove_file(&out)?;
    }
    let status = Command::new("truncate")
        .args(["-s", spec.size, out.to_string_lossy().as_ref()])
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
            spec.fs_label,
            out.to_string_lossy().as_ref(),
        ])
        .status()
        .context("mkfs.ext4 -d")?;
    if !status.success() {
        bail!("mkfs.ext4 failed");
    }

    // Sidecar checksum so hollow-vm can verify the bytes haven't been
    // swapped between here and the launch host.
    let sidecar = out.with_file_name(format!("{}.ext4.sha256", spec.name));
    let digest = sha256_file(&out)
        .with_context(|| format!("hash {}", out.display()))?;
    std::fs::write(&sidecar, format!("{digest}\n"))
        .with_context(|| format!("write {}", sidecar.display()))?;

    tracing::info!(image = spec.name, path = %out.display(), sha256 = %digest, "ext4 built");
    Ok(ImageArtifact {
        name: spec.name.to_string(),
        ext4_path: out,
    })
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut f = std::fs::File::open(path)?;
    std::io::copy(&mut f, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

fn newer_than(a: &Path, b: &Path) -> anyhow::Result<bool> {
    let am = std::fs::metadata(a)?.modified()?;
    let bm = std::fs::metadata(b)?.modified()?;
    Ok(am >= bm)
}
