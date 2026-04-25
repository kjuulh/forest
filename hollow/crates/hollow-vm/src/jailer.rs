//! Firecracker jailer integration.
//!
//! Instead of spawning Firecracker directly, we invoke `jailer`, which:
//!   - chroots Firecracker into `<chroot_base>/<fc_basename>/<id>/root/`
//!   - creates the required device nodes (`/dev/kvm`, `/dev/net/tun`,
//!     `/dev/urandom`) inside the chroot
//!   - drops into a `new mount namespace` (so the chroot is honest)
//!   - switches to an unprivileged UID/GID
//!   - optionally sets resource limits via cgroup v2
//!
//! The caller must pre-stage kernel + rootfs files into the chroot (we use
//! hardlinks so this is ~free on the same filesystem) and the Firecracker
//! API paths must be rewritten to be **chroot-relative**. Those two
//! translations are the main thing this module automates.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

/// Inputs the runtime needs to launch a jailed Firecracker. All paths are on
/// the host and may live anywhere readable by the caller (root at boot time).
/// We hardlink into the chroot, so keep them on the same filesystem as
/// `chroot_base` to avoid an `EXDEV` copy fallback.
#[derive(Debug, Clone)]
pub struct JailerConfig {
    pub jailer_bin: PathBuf,
    pub firecracker_bin: PathBuf,
    pub chroot_base: PathBuf,
    pub uid: u32,
    pub gid: u32,
}

/// Per-VM resource caps applied via cgroup v2. Without these, Firecracker is
/// configured with `vcpus`/`mem_mib` at the API level but the host process
/// itself is unconstrained — a VM that allocates aggressively or fork-bombs
/// inside its guest can starve the host.
#[derive(Debug, Clone)]
pub struct JailerLimits {
    /// Hard memory cap on the Firecracker process tree (cgroup memory.max).
    /// Sized to match the VM's `mem_size_mib` so the guest can't exceed
    /// what we told it it had.
    pub memory_max_bytes: u64,
    /// vCPUs available to the VM. Translated to cgroup cpu.max as
    /// `<vcpus * period> <period>` with `period = 100_000us`.
    pub vcpus: u8,
    /// Maximum tasks in the cgroup. Firecracker itself uses a handful of
    /// threads; the rest are jailer/init/job descendants. 512 is generous
    /// for normal workloads and tight enough to stop a fork-bomb.
    pub pids_max: u32,
}

impl JailerLimits {
    /// Derive limits from the VM's compute size, with sensible defaults for
    /// pids_max. Callers can override the resulting struct directly.
    pub fn from_vm_size(vcpus: u8, mem_mib: u32) -> Self {
        Self {
            memory_max_bytes: u64::from(mem_mib) * 1024 * 1024,
            vcpus: vcpus.max(1),
            pids_max: 512,
        }
    }
}

/// Resolved per-VM chroot layout. Produced by [`stage_chroot`] just before
/// jailer is invoked. Field values are always absolute host paths; [`in_chroot`]
/// converts any of them to chroot-relative form for Firecracker API calls.
#[derive(Debug, Clone)]
pub struct ChrootLayout {
    /// Unique VM id (passed to jailer as `--id`).
    pub vm_id: String,
    /// Absolute host path to `<chroot_base>/<fc_basename>/<id>/root/`.
    pub chroot_root: PathBuf,
    /// Host path to the kernel inside the chroot (e.g. `<chroot_root>/kernel`).
    pub kernel_host: PathBuf,
    /// Host path to the rootfs inside the chroot.
    pub rootfs_host: PathBuf,
    /// Where jailer will have Firecracker create its API UDS (inside chroot).
    pub api_sock_host: PathBuf,
    /// Where jailer will have Firecracker create its vsock UDS (inside chroot).
    pub vsock_uds_host: PathBuf,
    /// Host path to the Firecracker stdout log (jailer pipes stdout here).
    pub console_log_host: PathBuf,
    /// Basename of the Firecracker binary — jailer uses this to pick the
    /// chroot subdirectory name.
    pub firecracker_basename: String,
}

impl ChrootLayout {
    /// Convert an absolute host path under `chroot_root` to a chroot-relative
    /// path suitable for Firecracker API JSON.
    pub fn in_chroot(&self, host_path: &Path) -> anyhow::Result<String> {
        let rel = host_path
            .strip_prefix(&self.chroot_root)
            .with_context(|| {
                format!(
                    "{} is not under chroot_root {}",
                    host_path.display(),
                    self.chroot_root.display(),
                )
            })?;
        let mut s = String::from("/");
        s.push_str(&rel.to_string_lossy());
        Ok(s)
    }
}

/// Stage the chroot directory: create the tree, hardlink kernel + rootfs
/// into it, and compute all the paths the caller will need.
///
/// Does NOT invoke jailer — the caller wires up the process spawn separately.
/// That keeps this module easy to test and lets `VmInstance` own the actual
/// `Command::new` / `Child` lifecycle.
pub fn stage_chroot(
    cfg: &JailerConfig,
    vm_id: &str,
    kernel: &Path,
    rootfs: &Path,
) -> anyhow::Result<ChrootLayout> {
    let firecracker_basename = cfg
        .firecracker_bin
        .file_name()
        .and_then(|n| n.to_str())
        .context("firecracker_bin has no basename")?
        .to_string();

    // Jailer's fixed layout: <chroot_base>/<basename>/<id>/root/
    let chroot_root = cfg
        .chroot_base
        .join(&firecracker_basename)
        .join(vm_id)
        .join("root");

    // If a previous run left state behind (e.g. a panic before shutdown),
    // blow it away. Jailer will refuse to start with an existing chroot.
    if chroot_root.exists() {
        std::fs::remove_dir_all(cfg.chroot_base.join(&firecracker_basename).join(vm_id))
            .with_context(|| format!("remove stale chroot {}", chroot_root.display()))?;
    }
    std::fs::create_dir_all(&chroot_root).context("create chroot root")?;
    std::fs::create_dir_all(chroot_root.join("run")).context("create chroot /run")?;

    let kernel_host = chroot_root.join("kernel");
    hardlink_or_copy(kernel, &kernel_host).context("stage kernel")?;

    let rootfs_host = chroot_root.join("rootfs.ext4");
    hardlink_or_copy(rootfs, &rootfs_host).context("stage rootfs")?;

    // Jailer writes firecracker's stdout to <chroot_parent>/firecracker.log
    // but we want it predictable; let Firecracker use whatever the spawn
    // step configures and set this to the path we're going to redirect to.
    let console_log_host = chroot_root
        .parent()
        .context("chroot_root has no parent")?
        .join("firecracker.log");

    let api_sock_host = chroot_root.join("run/firecracker.sock");
    let vsock_uds_host = chroot_root.join("run/vsock.sock");

    // Own the chroot tree as the target uid/gid so Firecracker can write
    // inside it once jailer drops privileges.
    chown_recursive(&chroot_root, cfg.uid, cfg.gid).context("chown chroot tree")?;

    Ok(ChrootLayout {
        vm_id: vm_id.to_string(),
        chroot_root,
        kernel_host,
        rootfs_host,
        api_sock_host,
        vsock_uds_host,
        console_log_host,
        firecracker_basename,
    })
}

/// Clean up the chroot tree after the VM has stopped. Best-effort — if
/// Firecracker was killed mid-run, jailer's cgroup sweep usually removes
/// most of it, but there can be leftover files owned by the jailed UID.
pub fn teardown_chroot(cfg: &JailerConfig, layout: &ChrootLayout) {
    let vm_chroot_dir = cfg
        .chroot_base
        .join(&layout.firecracker_basename)
        .join(&layout.vm_id);
    if vm_chroot_dir.exists()
        && let Err(e) = std::fs::remove_dir_all(&vm_chroot_dir)
    {
        tracing::warn!(
            path = %vm_chroot_dir.display(),
            error = %e,
            "failed to remove chroot tree (may leak disk until next run)"
        );
    }
}

fn hardlink_or_copy(src: &Path, dst: &Path) -> anyhow::Result<()> {
    // Resolve symlinks first — if the source is a symlink (e.g. our image
    // aliases like `echo-v1.ext4 → base.ext4`), hard-linking it creates a
    // new symlink with the same *relative* target, which breaks once it's
    // inside the chroot. Canonicalize so we always stage the real file.
    let real_src = std::fs::canonicalize(src)
        .with_context(|| format!("canonicalize {}", src.display()))?;

    // Jailer is stricter about its chroot being clean, so don't silently
    // reuse a stale link — always replace.
    if dst.exists() || dst.symlink_metadata().is_ok() {
        std::fs::remove_file(dst).ok();
    }
    match std::fs::hard_link(&real_src, dst) {
        Ok(_) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            // Different filesystem → fall back to copy.
            std::fs::copy(&real_src, dst)
                .map(|_| ())
                .with_context(|| format!("copy {} → {}", real_src.display(), dst.display()))
        }
        Err(e) => Err(e).with_context(|| {
            format!("hardlink {} → {}", real_src.display(), dst.display())
        }),
    }
}

fn chown_recursive(root: &Path, uid: u32, gid: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::chown;
    chown(root, Some(uid), Some(gid))?;
    let iter = std::fs::read_dir(root).with_context(|| format!("read_dir {}", root.display()))?;
    for entry in iter {
        let entry = entry?;
        let ft = entry.file_type()?;
        let path = entry.path();
        if ft.is_dir() {
            chown_recursive(&path, uid, gid)?;
        } else {
            chown(&path, Some(uid), Some(gid))?;
        }
    }
    Ok(())
}

/// Build the argv for spawning jailer. The exec-file and chroot base come
/// from the [`JailerConfig`]; the remaining pieces come from the staged
/// [`ChrootLayout`]. Everything after `--` is passed through to Firecracker.
pub fn build_argv(
    cfg: &JailerConfig,
    layout: &ChrootLayout,
    limits: &JailerLimits,
) -> Vec<String> {
    let mut argv: Vec<String> = Vec::new();
    argv.extend([
        "--id".to_string(),
        layout.vm_id.clone(),
        "--exec-file".to_string(),
        cfg.firecracker_bin.to_string_lossy().into_owned(),
        "--uid".to_string(),
        cfg.uid.to_string(),
        "--gid".to_string(),
        cfg.gid.to_string(),
        "--chroot-base-dir".to_string(),
        cfg.chroot_base.to_string_lossy().into_owned(),
        // cgroup v2 unified hierarchy. Trixie defaults to v2; older distros
        // would need a kernel cmdline change.
        "--cgroup-version".to_string(),
        "2".to_string(),
    ]);

    // Per-VM resource caps. Each --cgroup flag is "<file>=<value>".
    // cpu.max takes "<quota> <period>" with a literal space — argv handles
    // it fine (no shell tokenisation).
    let cpu_period_us: u32 = 100_000;
    let cpu_quota_us: u32 = u32::from(limits.vcpus) * cpu_period_us;
    let cgroup_kv: Vec<String> = vec![
        format!("memory.max={}", limits.memory_max_bytes),
        // Forbid the VM's pages from being swapped — protects neighbouring
        // tenants' latency and avoids tenant data hitting persistent storage.
        "memory.swap.max=0".to_string(),
        format!("cpu.max={cpu_quota_us} {cpu_period_us}"),
        format!("pids.max={}", limits.pids_max),
    ];
    for kv in cgroup_kv {
        argv.push("--cgroup".to_string());
        argv.push(kv);
    }

    // Separator then Firecracker-specific args.
    argv.push("--".to_string());
    argv.push("--api-sock".to_string());
    // Inside the chroot, the API socket lives at /run/firecracker.sock.
    argv.push("/run/firecracker.sock".to_string());
    argv
}

/// Verify that the jailer binary is actually present. Saves us a confusing
/// `ENOENT` when the bootstrap didn't install it.
pub fn check_binary(cfg: &JailerConfig) -> anyhow::Result<()> {
    if !cfg.jailer_bin.exists() {
        bail!(
            "jailer binary not found at {} — bootstrap should download it alongside firecracker",
            cfg.jailer_bin.display()
        );
    }
    if !cfg.firecracker_bin.exists() {
        bail!(
            "firecracker binary not found at {}",
            cfg.firecracker_bin.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_layout() -> ChrootLayout {
        ChrootLayout {
            vm_id: "vm-deadbeef".into(),
            chroot_root: PathBuf::from("/var/lib/hollow/jailer/firecracker/vm-deadbeef/root"),
            kernel_host: PathBuf::from(
                "/var/lib/hollow/jailer/firecracker/vm-deadbeef/root/kernel",
            ),
            rootfs_host: PathBuf::from(
                "/var/lib/hollow/jailer/firecracker/vm-deadbeef/root/rootfs.ext4",
            ),
            api_sock_host: PathBuf::from(
                "/var/lib/hollow/jailer/firecracker/vm-deadbeef/root/run/firecracker.sock",
            ),
            vsock_uds_host: PathBuf::from(
                "/var/lib/hollow/jailer/firecracker/vm-deadbeef/root/run/vsock.sock",
            ),
            console_log_host: PathBuf::from(
                "/var/lib/hollow/jailer/firecracker/vm-deadbeef/firecracker.log",
            ),
            firecracker_basename: "firecracker".into(),
        }
    }

    fn fixture_config() -> JailerConfig {
        JailerConfig {
            jailer_bin: PathBuf::from("/usr/bin/jailer"),
            firecracker_bin: PathBuf::from("/usr/bin/firecracker"),
            chroot_base: PathBuf::from("/var/lib/hollow/jailer"),
            uid: 10000,
            gid: 10000,
        }
    }

    #[test]
    fn limits_from_vm_size_scales_correctly() {
        let l = JailerLimits::from_vm_size(2, 1024);
        assert_eq!(l.memory_max_bytes, 2u64.pow(30)); // 1024 MiB == 2^30
        assert_eq!(l.vcpus, 2);
        assert_eq!(l.pids_max, 512);

        // Floor to 1 vCPU even if caller passes 0 — cpu.max=0 would deadlock.
        let l = JailerLimits::from_vm_size(0, 256);
        assert_eq!(l.vcpus, 1);
    }

    #[test]
    fn build_argv_includes_cgroup_limits() {
        let argv = build_argv(
            &fixture_config(),
            &fixture_layout(),
            &JailerLimits::from_vm_size(2, 1024),
        );
        // cgroup-version
        assert!(argv.windows(2).any(|w| w[0] == "--cgroup-version" && w[1] == "2"));
        // memory.max in bytes
        assert!(argv.contains(&"memory.max=1073741824".to_string()));
        // cpu.max as "<quota> <period>" with a literal space
        assert!(argv.contains(&"cpu.max=200000 100000".to_string()));
        // pids.max
        assert!(argv.contains(&"pids.max=512".to_string()));
        // swap forbidden — VM data must not hit persistent storage
        assert!(argv.contains(&"memory.swap.max=0".to_string()));
        // every cgroup option is paired with --cgroup
        let cgroup_flag_count = argv.iter().filter(|a| *a == "--cgroup").count();
        let cgroup_kv_count = argv
            .iter()
            .filter(|a| {
                a.contains("memory.max=")
                    || a.contains("memory.swap.max=")
                    || a.contains("cpu.max=")
                    || a.contains("pids.max=")
            })
            .count();
        assert_eq!(cgroup_flag_count, cgroup_kv_count);
    }
}
