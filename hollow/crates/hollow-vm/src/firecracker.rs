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
    process: Option<Child>,
    client: hyper_util::client::legacy::Client<UnixConnector, Full<Bytes>>,
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
struct Action<'a> {
    action_type: &'a str,
}

impl VmInstance {
    /// Spawn the Firecracker process bound to a fresh API socket inside `workdir`.
    /// Does not configure or start the VM yet — call the `put_*` methods then
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
            process: Some(child),
            client,
        })
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
        self.put(
            "/boot-source",
            &BootSource {
                kernel_image_path,
                boot_args,
            },
        )
        .await
    }

    pub async fn put_root_drive(&self, rootfs_path: &str, read_only: bool) -> anyhow::Result<()> {
        self.put(
            "/drives/rootfs",
            &DriveConfig {
                drive_id: "rootfs",
                path_on_host: rootfs_path,
                is_root_device: true,
                is_read_only: read_only,
            },
        )
        .await
    }

    pub async fn put_vsock(&self, guest_cid: u32) -> anyhow::Result<()> {
        let uds = self.vsock_uds.to_string_lossy().to_string();
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
        // Firecracker creates per-port suffixed UDS files; clean the whole dir.
        clean_vsock_dir(&self.workdir).await;
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

async fn clean_vsock_dir(workdir: &Path) {
    if let Ok(mut entries) = tokio::fs::read_dir(workdir).await {
        while let Ok(Some(e)) = entries.next_entry().await {
            let name = e.file_name();
            let n = name.to_string_lossy();
            if n.starts_with("vsock.sock") {
                let _ = tokio::fs::remove_file(e.path()).await;
            }
        }
    }
}
