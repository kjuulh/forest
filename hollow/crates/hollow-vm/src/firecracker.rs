//! Thin Firecracker API client.
//!
//! Firecracker speaks HTTP over a Unix domain socket. We send a fixed
//! sequence of PUTs (machine-config, boot-source, drive, vsock) and then
//! POST `/actions {"action_type": "InstanceStart"}` to boot the VM.
//!
//! The whole client is bespoke and minimal because we only ever issue a handful
//! of requests per VM. Using a generic OpenAPI-generated client would pull in
//! more deps than the entire rest of the runner.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, bail};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Method, Request};
use hyperlocal::{UnixClientExt, UnixConnector, Uri as UnixUri};
use serde::Serialize;
use tokio::process::{Child, Command};

use crate::jailer::{self, ChrootLayout, JailerConfig};

/// Encapsulates a single Firecracker VM instance — its API socket, vsock UDS,
/// and launched process. Drop ensures the process is killed and sockets removed.
pub struct VmInstance {
    pub workdir: PathBuf,
    pub api_sock: PathBuf,
    pub vsock_uds: PathBuf,
    /// Firecracker stdout log — captures the guest's serial console, which is
    /// where the kernel prints dmesg and `hollow-guest` (running as PID 1)
    /// writes its tracing output. Populated in [`spawn`](Self::spawn) and read
    /// after shutdown via [`read_console_log`](Self::read_console_log).
    pub console_log: PathBuf,
    /// When the VM is jailed, every API path the caller passes to the
    /// `put_*` methods is rewritten to be chroot-relative, and the host-side
    /// paths already on this struct (`api_sock`, `vsock_uds`) point inside
    /// the chroot.
    jailer: Option<JailerRuntime>,
    process: Option<Child>,
    client: hyper_util::client::legacy::Client<UnixConnector, Full<Bytes>>,
}

/// Per-VM jailer state carried by [`VmInstance`] so the `put_*` methods can
/// rewrite paths and so teardown removes the chroot tree.
struct JailerRuntime {
    config: JailerConfig,
    layout: ChrootLayout,
}

#[derive(Serialize)]
struct MachineConfig {
    vcpu_count: u8,
    mem_size_mib: u32,
}

#[derive(Serialize)]
struct BootSource<'a> {
    kernel_image_path: &'a str,
    boot_args: &'a str,
}

#[derive(Serialize)]
struct DriveConfig<'a> {
    drive_id: &'a str,
    path_on_host: &'a str,
    is_root_device: bool,
    is_read_only: bool,
}

#[derive(Serialize)]
struct VsockConfig<'a> {
    guest_cid: u32,
    uds_path: &'a str,
}

#[derive(Serialize)]
struct NetworkInterface<'a> {
    iface_id: &'a str,
    host_dev_name: &'a str,
    guest_mac: &'a str,
}

#[derive(Serialize)]
struct Action<'a> {
    action_type: &'a str,
}

impl VmInstance {
    /// Spawn Firecracker directly (no jailer). Intended for development and
    /// diagnostics; production should always go through [`spawn_jailed`].
    /// The VM is not yet configured — call the `put_*` methods then
    /// [`start`](Self::start).
    pub async fn spawn(firecracker_bin: &Path, workdir: PathBuf) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(&workdir)
            .await
            .context("create vm workdir")?;
        let api_sock = workdir.join("firecracker.sock");
        let vsock_uds = workdir.join("vsock.sock");

        // Stale socket from a previous run will block bind.
        let _ = tokio::fs::remove_file(&api_sock).await;
        let _ = tokio::fs::remove_file(&vsock_uds).await;

        let log_file = workdir.join("firecracker.log");
        let stderr_file = workdir.join("firecracker.stderr.log");

        let child = Command::new(firecracker_bin)
            .arg("--api-sock")
            .arg(&api_sock)
            .arg("--id")
            .arg(format!("vm-{}", uuid::Uuid::new_v4()))
            .stdin(Stdio::null())
            .stdout(Stdio::from(std::fs::File::create(&log_file)?))
            .stderr(Stdio::from(std::fs::File::create(&stderr_file)?))
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn {}", firecracker_bin.display()))?;

        // Wait for the API socket to appear so subsequent requests don't ECONNREFUSED.
        wait_for_path(&api_sock, Duration::from_secs(5))
            .await
            .context("firecracker did not create api socket")?;

        let client = hyper_util::client::legacy::Client::unix();

        Ok(Self {
            workdir,
            api_sock,
            vsock_uds,
            console_log: log_file,
            jailer: None,
            process: Some(child),
            client,
        })
    }

    /// Spawn Firecracker inside a jailer chroot with privilege drop.
    ///
    /// Kernel + rootfs are hardlinked (or copied) into the chroot up-front;
    /// the paths surfaced on [`VmInstance`] are absolute host paths under the
    /// chroot root, and subsequent `put_*` calls are transparently rewritten
    /// to be chroot-relative.
    pub async fn spawn_jailed(
        cfg: &JailerConfig,
        workdir: PathBuf,
        kernel: &Path,
        rootfs: &Path,
    ) -> anyhow::Result<Self> {
        jailer::check_binary(cfg)?;
        tokio::fs::create_dir_all(&workdir)
            .await
            .context("create vm workdir")?;

        // Short vm_id to keep the chroot path inside SUN_LEN. Jailer's chroot
        // path includes this verbatim, and we add `/root/run/firecracker.sock`
        // (~25 bytes) on top.
        let vm_id = short_vm_id();
        let layout = jailer::stage_chroot(cfg, &vm_id, kernel, rootfs)?;

        // Jailer pipes the jailed Firecracker's stdout/stderr to its own
        // stdout/stderr, so we capture them the same way we do for the
        // non-jailed path. This doubles as our console capture path.
        let stdout_log = workdir.join("firecracker.log");
        let stderr_log = workdir.join("firecracker.stderr.log");

        let argv = jailer::build_argv(cfg, &layout);
        tracing::info!(
            jailer = %cfg.jailer_bin.display(),
            argv = ?argv,
            "spawning jailer"
        );

        let child = Command::new(&cfg.jailer_bin)
            .args(&argv)
            .stdin(Stdio::null())
            .stdout(Stdio::from(std::fs::File::create(&stdout_log)?))
            .stderr(Stdio::from(std::fs::File::create(&stderr_log)?))
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn {}", cfg.jailer_bin.display()))?;

        // Jailer creates the chroot + spawns Firecracker, which then creates
        // the API socket at /run/firecracker.sock inside the chroot. Wait for
        // that to appear (slightly longer than the non-jailed path to account
        // for the extra namespace/chroot setup).
        wait_for_path(&layout.api_sock_host, Duration::from_secs(10))
            .await
            .context("jailed firecracker did not create api socket")?;

        let client = hyper_util::client::legacy::Client::unix();

        Ok(Self {
            workdir,
            api_sock: layout.api_sock_host.clone(),
            vsock_uds: layout.vsock_uds_host.clone(),
            console_log: stdout_log,
            jailer: Some(JailerRuntime {
                config: cfg.clone(),
                layout,
            }),
            process: Some(child),
            client,
        })
    }

    /// If this VM is jailed, the caller-supplied host path is hardlinked into
    /// the chroot (if not already) and the chroot-relative path is returned.
    /// Otherwise the input is returned unchanged.
    fn api_path_for(&self, host_path: &Path) -> anyhow::Result<String> {
        match &self.jailer {
            Some(j) => {
                // Files that we already staged during `spawn_jailed` are
                // under chroot_root and in_chroot will just trim the prefix.
                j.layout.in_chroot(host_path)
            }
            None => Ok(host_path.to_string_lossy().into_owned()),
        }
    }

    /// Read the guest serial console output captured to Firecracker's stdout
    /// (kernel dmesg + anything hollow-guest prints). Safe to call at any
    /// point; most useful AFTER [`shutdown`](Self::shutdown) so all buffered
    /// output has flushed.
    pub async fn read_console_log(&self) -> anyhow::Result<String> {
        tokio::fs::read_to_string(&self.console_log)
            .await
            .with_context(|| format!("read {}", self.console_log.display()))
    }

    pub async fn put_machine_config(&self, vcpus: u8, mem_mib: u32) -> anyhow::Result<()> {
        self.put(
            "/machine-config",
            &MachineConfig {
                vcpu_count: vcpus,
                mem_size_mib: mem_mib,
            },
        )
        .await
    }

    pub async fn put_boot_source(
        &self,
        kernel_image_path: &str,
        boot_args: &str,
    ) -> anyhow::Result<()> {
        // When jailed, kernel was staged into the chroot during spawn — use
        // the chroot-relative path Firecracker can actually open from inside
        // the chroot, regardless of what host path the caller passed.
        let api_path = match &self.jailer {
            Some(j) => j.layout.in_chroot(&j.layout.kernel_host)?,
            None => kernel_image_path.to_string(),
        };
        self.put(
            "/boot-source",
            &BootSource {
                kernel_image_path: &api_path,
                boot_args,
            },
        )
        .await
    }

    pub async fn put_root_drive(&self, rootfs_path: &str, read_only: bool) -> anyhow::Result<()> {
        let api_path = match &self.jailer {
            Some(j) => j.layout.in_chroot(&j.layout.rootfs_host)?,
            None => rootfs_path.to_string(),
        };
        self.put(
            "/drives/rootfs",
            &DriveConfig {
                drive_id: "rootfs",
                path_on_host: &api_path,
                is_root_device: true,
                is_read_only: read_only,
            },
        )
        .await
    }

    /// Attach a tap-backed virtio-net device. The tap must already exist on
    /// the host (see `hollow_vm::net`).
    pub async fn put_network_interface(
        &self,
        iface_id: &str,
        host_dev_name: &str,
        guest_mac: &str,
    ) -> anyhow::Result<()> {
        let path = format!("/network-interfaces/{iface_id}");
        self.put(
            &path,
            &NetworkInterface {
                iface_id,
                host_dev_name,
                guest_mac,
            },
        )
        .await
    }

    pub async fn put_vsock(&self, guest_cid: u32) -> anyhow::Result<()> {
        // vsock_uds is a host path; when jailed, translate to in-chroot form.
        let uds = self.api_path_for(&self.vsock_uds)?;
        self.put(
            "/vsock",
            &VsockConfig {
                guest_cid,
                uds_path: &uds,
            },
        )
        .await
    }

    pub async fn start(&self) -> anyhow::Result<()> {
        self.put(
            "/actions",
            &Action {
                action_type: "InstanceStart",
            },
        )
        .await
    }

    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        // SendCtrlAltDel is x86-only; for our minimal kernel without acpi support
        // we just kill the firecracker process — VM exits, sockets close.
        if let Some(mut child) = self.process.take() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        }
        let _ = tokio::fs::remove_file(&self.api_sock).await;
        // Firecracker creates per-port suffixed UDS files; clean the parent
        // of the vsock_uds (which is the /run dir in the chroot when jailed,
        // or the workdir otherwise).
        if let Some(parent) = self.vsock_uds.parent() {
            clean_vsock_dir(parent).await;
        }
        // Remove the jailer chroot tree now that Firecracker is dead.
        if let Some(j) = &self.jailer {
            jailer::teardown_chroot(&j.config, &j.layout);
        }
        Ok(())
    }

    async fn put<T: Serialize>(&self, path: &str, body: &T) -> anyhow::Result<()> {
        let payload = serde_json::to_vec(body)?;
        let uri: hyper::Uri = UnixUri::new(&self.api_sock, path).into();
        let req = Request::builder()
            .method(Method::PUT)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(payload)))
            .context("build firecracker request")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("PUT {path}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp
                .into_body()
                .collect()
                .await
                .map(|c| c.to_bytes())
                .unwrap_or_default();
            bail!(
                "firecracker {} failed: {} — {}",
                path,
                status,
                String::from_utf8_lossy(&body)
            );
        }
        Ok(())
    }
}

impl Drop for VmInstance {
    fn drop(&mut self) {
        if let Some(mut child) = self.process.take() {
            let _ = child.start_kill();
        }
    }
}

/// Short collision-resistant VM id (8 hex chars from a fresh UUID).
fn short_vm_id() -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    format!("vm-{}", &u[..8])
}

async fn wait_for_path(path: &Path, timeout: Duration) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if tokio::fs::metadata(path).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    bail!("timed out waiting for {}", path.display())
}

async fn clean_vsock_dir(dir: &Path) {
    if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(e)) = entries.next_entry().await {
            let name = e.file_name();
            let n = name.to_string_lossy();
            if n.starts_with("vsock.sock") {
                let _ = tokio::fs::remove_file(e.path()).await;
            }
        }
    }
}
