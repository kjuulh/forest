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
    pub jailer_bin: String,
    pub kernel: String,
    pub rootfs: String,
    pub runner_bin: String,
    pub agent_bin: String,
    pub images_dir: String,
    pub workdir_root: String,
    pub agent_data_dir: String,
    /// Per-VM chroot base (jailer expands this to
    /// `<base>/<fc-basename>/<vm-id>/root/`).
    pub jailer_chroot_base: String,
    /// UID/GID Firecracker drops to under jailer.
    pub jailer_uid: u32,
    pub jailer_gid: u32,
    /// Default-route interface on the remote host (used as MASQUERADE out).
    pub host_iface: String,
}

pub fn bootstrap(cfg: &Config, artifacts: &BuildArtifacts) -> anyhow::Result<RemoteLayout> {
    // 1. Verify host capability and create directory layout.
    preflight(cfg)?;

    let bin_dir = format!("{}/bin", cfg.remote_dir.display());
    let images_dir = format!("{}/images", cfg.remote_dir.display());
    let workdir_root = format!("{}/work", cfg.remote_dir.display());

    // No version suffix in the binary names — jailer uses the firecracker
    // basename as a path component in the chroot, and the chroot path has to
    // fit inside SUN_LEN (~108 bytes) when we add /run/firecracker.sock to it.
    // Using a short fixed name keeps us safely under that limit.
    let firecracker_bin = format!("{bin_dir}/firecracker");
    let jailer_bin = format!("{bin_dir}/jailer");
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
    let jailer_chroot_base = format!("{}/jailer", cfg.remote_dir.display());

    // 2. Install firecracker + jailer (single tarball; download once).
    install_firecracker(cfg, &firecracker_bin, &jailer_bin)?;

    // 3. Install kernel.
    install_kernel(cfg, &kernel)?;

    // 4. Ship the runner and agent binaries.
    ship_artifact(cfg, &artifacts.runner_bin, &runner_bin, /* exec */ true)?;
    ship_artifact(cfg, &artifacts.agent_bin, &agent_bin, /* exec */ true)?;

    // 5. Ship every rootfs image built locally.
    for img in &artifacts.images {
        let remote_path = format!("{images_dir}/{}.ext4", img.name);
        ship_artifact(cfg, &img.ext4_path, &remote_path, /* exec */ false)?;
    }

    // 6. Provide image aliases so the controller's `{dest}-v{ver}.ext4`
    //    naming convention finds the right ext4. One alias per destination
    //    type that maps to an existing image. Add arms here for new
    //    destination types.
    let script = format!(
        r#"set -e
cd {images_dir}
link_if_missing() {{
  target="$1"; link="$2"
  if [ -f "$target" ] && [ ! -e "$link" ]; then
    ln -sf "$target" "$link"
  fi
}}
link_if_missing base.ext4 echo-v1.ext4
link_if_missing base.ext4 base-v1.ext4
mkdir -p {agent_data_dir}
"#
    );
    ssh::run_remote(cfg, &script).context("install image aliases")?;

    // 7. Detect the remote's outbound interface and enable ip_forward.
    let host_iface = detect_host_iface(cfg).context("detect remote host outbound iface")?;
    let forward_script = format!(
        r#"set -e
echo 1 > /proc/sys/net/ipv4/ip_forward
mkdir -p {jailer_chroot_base}
"#
    );
    ssh::run_remote(cfg, &forward_script).context("enable ip_forward + jailer dirs")?;

    // Jailer numeric UID/GID. Linux setresuid/setresgid don't require these
    // to exist in /etc/passwd, so we can use a fixed pair without managing
    // system users on the remote.
    let jailer_uid = 10000;
    let jailer_gid = 10000;

    Ok(RemoteLayout {
        firecracker_bin,
        jailer_bin,
        kernel,
        rootfs,
        runner_bin,
        agent_bin,
        images_dir,
        workdir_root,
        agent_data_dir,
        jailer_chroot_base,
        jailer_uid,
        jailer_gid,
        host_iface,
    })
}

fn detect_host_iface(cfg: &Config) -> anyhow::Result<String> {
    let out = ssh::run_remote(
        cfg,
        "ip -4 route show default | awk '/^default/ {for (i=1;i<=NF;i++) if ($i==\"dev\") print $(i+1)}'",
    )?;
    let iface = out.trim().lines().next().unwrap_or("").trim();
    if iface.is_empty() {
        bail!("could not detect default-route interface on remote");
    }
    Ok(iface.to_string())
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

fn install_firecracker(
    cfg: &Config,
    firecracker_target: &str,
    jailer_target: &str,
) -> anyhow::Result<()> {
    let url = FIRECRACKER_DOWNLOAD_TEMPLATE.replace("{ver}", cfg.firecracker_version);
    // The release archive bundles both binaries:
    //   release-vX.Y.Z-x86_64/firecracker-vX.Y.Z-x86_64
    //   release-vX.Y.Z-x86_64/jailer-vX.Y.Z-x86_64
    let inner_fc = format!(
        "release-{ver}-x86_64/firecracker-{ver}-x86_64",
        ver = cfg.firecracker_version
    );
    let inner_jailer = format!(
        "release-{ver}-x86_64/jailer-{ver}-x86_64",
        ver = cfg.firecracker_version
    );
    let script = format!(
        r#"set -e
if [ -x "{fc_target}" ] && [ -x "{jl_target}" ]; then exit 0; fi
tmp=$(mktemp -d)
trap "rm -rf $tmp" EXIT
echo "downloading firecracker {ver} (incl. jailer)..." >&2
curl -fsSL "{url}" -o "$tmp/fc.tgz"
tar -xzf "$tmp/fc.tgz" -C "$tmp" "{inner_fc}" "{inner_jailer}"
install -m 0755 "$tmp/{inner_fc}" "{fc_target}"
install -m 0755 "$tmp/{inner_jailer}" "{jl_target}"
"#,
        ver = cfg.firecracker_version,
        url = url,
        inner_fc = inner_fc,
        inner_jailer = inner_jailer,
        fc_target = firecracker_target,
        jl_target = jailer_target,
    );
    ssh::run_remote(cfg, &script).context("install firecracker + jailer")?;
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
