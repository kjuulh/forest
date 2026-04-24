//! Idempotent remote bootstrap. Run once per fresh host (or whenever the
//! pinned versions change). All steps short-circuit if the artifact is already
//! present at the expected path.

use std::path::Path;

use anyhow::{Context, bail};

use crate::build::BuildArtifacts;
use crate::config::Config;
use crate::ssh;

const FIRECRACKER_DOWNLOAD_TEMPLATE: &str =
    "https://github.com/firecracker-microvm/firecracker/releases/download/{ver}/firecracker-{ver}-x86_64.tgz";
const KERNEL_DOWNLOAD_BASE: &str = "https://s3.amazonaws.com/spec.ccfc.min";

/// Paths on the remote host after bootstrap completes.
#[derive(Debug, Clone)]
pub struct RemoteLayout {
    pub firecracker_bin: String,
    pub kernel: String,
    pub rootfs: String,
    pub runner_bin: String,
    pub agent_bin: String,
    pub images_dir: String,
    pub workdir_root: String,
    pub agent_data_dir: String,
}

pub fn bootstrap(cfg: &Config, artifacts: &BuildArtifacts) -> anyhow::Result<RemoteLayout> {
    // 1. Verify host capability and create directory layout.
    preflight(cfg)?;

    let bin_dir = format!("{}/bin", cfg.remote_dir.display());
    let images_dir = format!("{}/images", cfg.remote_dir.display());
    let workdir_root = format!("{}/work", cfg.remote_dir.display());

    let firecracker_bin = format!("{bin_dir}/firecracker-{}", cfg.firecracker_version);
    let runner_bin = format!("{bin_dir}/hollow-test-runner");
    let agent_bin = format!("{bin_dir}/hollow-agent");
    let kernel_filename = cfg
        .kernel_s3_key
        .rsplit('/')
        .next()
        .context("malformed kernel_s3_key")?;
    let kernel = format!("{bin_dir}/{kernel_filename}");
    let rootfs = format!("{images_dir}/base.ext4");
    let agent_data_dir = format!("{}/agent-data", cfg.remote_dir.display());

    // 2. Install firecracker (download + extract if missing).
    install_firecracker(cfg, &firecracker_bin)?;

    // 3. Install kernel.
    install_kernel(cfg, &kernel)?;

    // 4. Ship the runner, agent, and rootfs image.
    ship_artifact(cfg, &artifacts.runner_bin, &runner_bin, /* exec */ true)?;
    ship_artifact(cfg, &artifacts.agent_bin, &agent_bin, /* exec */ true)?;
    ship_artifact(cfg, &artifacts.rootfs_ext4, &rootfs, /* exec */ false)?;

    // 5. Provide image aliases so the controller's `{dest}-v{ver}.ext4`
    //    naming convention finds base.ext4. Add more aliases here as new
    //    destination types come online.
    let script = format!(
        r#"set -e
cd {images_dir}
for alias in echo-v1 base-v1; do
  if [ ! -e "$alias.ext4" ]; then
    ln -sf base.ext4 "$alias.ext4"
  fi
done
mkdir -p {agent_data_dir}
"#
    );
    ssh::run_remote(cfg, &script).context("install image aliases")?;

    Ok(RemoteLayout {
        firecracker_bin,
        kernel,
        rootfs,
        runner_bin,
        agent_bin,
        images_dir,
        workdir_root,
        agent_data_dir,
    })
}

fn preflight(cfg: &Config) -> anyhow::Result<()> {
    let script = format!(
        r#"set -e
mkdir -p {root}/bin {root}/images {root}/work
if [ ! -e /dev/kvm ]; then
  echo "FATAL: /dev/kvm missing on host — cannot run Firecracker." >&2
  exit 2
fi
if [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; then
  echo "FATAL: /dev/kvm is not r/w accessible to $(whoami)." >&2
  exit 2
fi
echo OK
"#,
        root = cfg.remote_dir.display()
    );
    let out = ssh::run_remote(cfg, &script).context("preflight failed")?;
    if !out.contains("OK") {
        bail!("unexpected preflight output: {out}");
    }
    Ok(())
}

fn install_firecracker(cfg: &Config, target: &str) -> anyhow::Result<()> {
    let url = FIRECRACKER_DOWNLOAD_TEMPLATE.replace("{ver}", cfg.firecracker_version);
    // Extract single binary from tarball; the release archive contains
    // `release-vX.Y.Z-x86_64/firecracker-vX.Y.Z-x86_64`.
    let inner = format!(
        "release-{ver}-x86_64/firecracker-{ver}-x86_64",
        ver = cfg.firecracker_version
    );
    let script = format!(
        r#"set -e
if [ -x "{target}" ]; then exit 0; fi
tmp=$(mktemp -d)
trap "rm -rf $tmp" EXIT
echo "downloading firecracker {ver}..." >&2
curl -fsSL "{url}" -o "$tmp/fc.tgz"
tar -xzf "$tmp/fc.tgz" -C "$tmp" "{inner}"
install -m 0755 "$tmp/{inner}" "{target}"
"#,
        ver = cfg.firecracker_version,
        url = url,
        inner = inner,
        target = target,
    );
    ssh::run_remote(cfg, &script).context("install firecracker")?;
    Ok(())
}

fn install_kernel(cfg: &Config, target: &str) -> anyhow::Result<()> {
    let url = format!("{}/{}", KERNEL_DOWNLOAD_BASE, cfg.kernel_s3_key);
    let script = format!(
        r#"set -e
if [ -f "{target}" ]; then exit 0; fi
echo "downloading kernel {key}..." >&2
curl -fsSL "{url}" -o "{target}.tmp"
mv "{target}.tmp" "{target}"
"#,
        target = target,
        url = url,
        key = cfg.kernel_s3_key,
    );
    ssh::run_remote(cfg, &script).context("install kernel")?;
    Ok(())
}

fn ship_artifact(
    cfg: &Config,
    local: &Path,
    remote: &str,
    executable: bool,
) -> anyhow::Result<()> {
    ssh::rsync_to(cfg, local, remote)?;
    if executable {
        ssh::run_remote(cfg, &format!("chmod 755 {remote}"))?;
    }
    Ok(())
}
