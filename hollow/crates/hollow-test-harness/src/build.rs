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
    /// Additional files inside `hollow/images/` whose mtime should
    /// invalidate the cache. Use for anything `COPY`ed by the Dockerfile
    /// — without this, editing e.g. `forest-flux-deploy` won't trigger
    /// a rebuild because the Dockerfile mtime hasn't moved.
    extra_inputs: &'static [&'static str],
}

const IMAGES: &[ImageBuild] = &[
    ImageBuild {
        name: "base",
        dockerfile: "Dockerfile.base",
        tag: "hollow-base:test",
        fs_label: "hollow-base",
        size: "256M",
        extra_inputs: &[],
    },
    ImageBuild {
        // Forest's destination registry calls this "terraform" (the binary
        // inside is actually OpenTofu — see Dockerfile for the rationale).
        name: "terraform-v1",
        dockerfile: "Dockerfile.terraform-v1",
        tag: "hollow-terraform-v1:test",
        fs_label: "hollow-tf",
        size: "1024M",
        extra_inputs: &[],
    },
    ImageBuild {
        // Forest's destination type is `forest/fluxv1/1` — controller maps
        // this to image `fluxv1-v1.ext4`. Ships git + openssh-client + flux
        // CLI + kustomize CLI plus the `forest-flux-deploy` workflow script.
        name: "fluxv1-v1",
        dockerfile: "Dockerfile.fluxv1",
        tag: "hollow-fluxv1-v1:test",
        fs_label: "hollow-flux",
        size: "1024M",
        extra_inputs: &["forest-flux-deploy"],
    },
    ImageBuild {
        // forest/exec/1 — general-purpose CUE-driven workflow runner.
        // v0 ships cue + jq + git + gh + bash; v1 will add podman for
        // `uses:` container actions once we've verified the kernel-side
        // primitives work in Firecracker.
        name: "exec-v1",
        dockerfile: "Dockerfile.exec-v1",
        tag: "hollow-exec-v1:test",
        fs_label: "hollow-exec",
        size: "2048M",
        extra_inputs: &[
            "forest-exec-runner",
            "podman-storage.conf",
            "podman-containers.conf",
            "forest-component-init",
            "forest-component-script",
            "forest-component-render-template",
            // Each script-component contributes its directory; if any
            // file inside a component changes we want a rebuild.
            "components/git-init/component",
            "components/git-init/manifest.json",
            "components/git-init/scripts/git-init.sh",
        ],
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

    // Native Forest components — compiled for musl so they're fully static
    // and portable into the alpine-based exec-v1 image. Each component
    // becomes a binary under /usr/local/lib/forest-components/<name>/<v>/
    // inside the image; the exec runner resolves `uses: forest:NAME@VER`
    // to that path. Staged into hollow/images/ so the Dockerfile can COPY
    // them in deterministically. We keep the staged file across builds so
    // its mtime matches the source — copying every run would invalidate
    // the ext4 cache check on every harness invocation.
    let init_bin = build_cargo(cfg, "forest-component-init", GUEST_TARGET, true)?;
    let staged_init = cfg
        .repo_root
        .join("images")
        .join("forest-component-init");
    stage_if_changed(&init_bin, &staged_init)?;

    // Generic script-component engine: same compile pattern, lives at
    // /usr/local/lib/forest-components/_engine/ inside the image. Lets
    // us ship new components as "drop a directory of shell scripts"
    // rather than a Rust crate per action.
    let script_engine_bin =
        build_cargo(cfg, "forest-component-script", GUEST_TARGET, true)?;
    let staged_script_engine = cfg
        .repo_root
        .join("images")
        .join("forest-component-script");
    stage_if_changed(&script_engine_bin, &staged_script_engine)?;

    // Forest-workspace components — proper components living under
    // `components/forest-contrib/<name>/crates/<name>/`, registered in
    // the forest root Cargo.toml. We compile from there (not the hollow
    // workspace) so the components are first-class Forest projects with
    // their own CUE specs, codegen output dirs, and registry metadata.
    let render_template_bin = build_cargo_forest(cfg, "render-template", GUEST_TARGET)?;
    let staged_render_template = cfg
        .repo_root
        .join("images")
        .join("forest-component-render-template");
    stage_if_changed(&render_template_bin, &staged_render_template)?;

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

/// Build a crate from the *parent* (forest) workspace, not the hollow
/// one. Forest components live at `forest/components/<org>/<name>/crates/`,
/// registered in `forest/Cargo.toml` workspace.members. Their target dir
/// is `forest/target/<target>/release/<pkg>`.
fn build_cargo_forest(cfg: &Config, pkg: &str, target: &str) -> anyhow::Result<PathBuf> {
    let forest_root = cfg
        .repo_root
        .parent()
        .ok_or_else(|| anyhow::anyhow!("hollow root has no parent (expected forest workspace)"))?;
    ensure_rustup_target(target)?;

    tracing::info!(pkg, target, "cargo build (release, forest workspace)");
    let status = Command::new("cargo")
        .current_dir(forest_root)
        .args(["build", "-p", pkg, "--release", "--target", target])
        .status()
        .with_context(|| format!("spawn cargo build for {pkg} (forest workspace)"))?;
    if !status.success() {
        bail!("cargo build {pkg} ({target}) in forest workspace failed");
    }

    let bin_path = forest_root
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
    // dockerfile or any explicitly-declared sidecar input changes.
    //
    // We short-circuit on `out.exists()` first because newer_than tries to
    // stat both arguments — if the ext4 hasn't been built yet, `out` is
    // missing and we'd surface a confusing "No such file or directory".
    let cache_ok = if out.exists() && newer_than(&out, &dockerfile)? && newer_than(&out, guest_bin)?
    {
        spec.extra_inputs
            .iter()
            .try_fold(true, |acc, name| -> anyhow::Result<bool> {
                Ok(acc && newer_than(&out, &images_dir.join(name))?)
            })?
    } else {
        false
    };
    if cache_ok {
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

/// Copy `src` to `dst` only when content differs, preserving `dst`'s mtime
/// when unchanged so downstream cache checks (`newer_than`) don't pointlessly
/// invalidate. Idempotent and cheap on the no-change path.
fn stage_if_changed(src: &Path, dst: &Path) -> anyhow::Result<()> {
    let src_meta = std::fs::metadata(src)
        .with_context(|| format!("stat {}", src.display()))?;
    if let Ok(dst_meta) = std::fs::metadata(dst)
        && dst_meta.len() == src_meta.len()
    {
        let src_bytes = std::fs::read(src)
            .with_context(|| format!("read {}", src.display()))?;
        let dst_bytes = std::fs::read(dst)
            .with_context(|| format!("read {}", dst.display()))?;
        if src_bytes == dst_bytes {
            return Ok(());
        }
    }
    std::fs::copy(src, dst)
        .with_context(|| format!("copy {} → {}", src.display(), dst.display()))?;
    Ok(())
}
