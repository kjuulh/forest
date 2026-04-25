//! hollow-guest: minimal agent running inside a Firecracker microVM.
//!
//! Connects to the host agent over vsock, receives a job definition,
//! executes the command, streams stdout/stderr back, and reports the exit code.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use hollow_vsock::protocol::{CompletionMsg, JobDefinition, LogLineMsg, Message};
use hollow_vsock::transport;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStdout;
use tokio::sync::mpsc;
use tokio_vsock::{VsockAddr, VsockStream};

/// Working directory inside the VM for job files.
const WORK_DIR: &str = "/work";

/// vsock CID of the host. Firecracker exposes the host as CID=2.
const HOST_CID: u32 = 2;

/// Default vsock port the agent listens on for guest connections.
const DEFAULT_PORT: u32 = 1024;

/// How long the guest will keep retrying to connect to the host on boot.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const CONNECT_RETRY: std::time::Duration = std::time::Duration::from_millis(200);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("hollow-guest starting");

    // Bring up the writable scratch areas the rest of the runtime expects
    // BEFORE we connect to the host. Rootfs is read-only; this gives the
    // job a place to write without tainting the underlying ext4.
    if let Err(e) = setup_writable_areas() {
        tracing::warn!(error = %e, "failed to mount tmpfs scratch areas (continuing)");
    }

    let stream = connect().await.context("failed to connect to host agent")?;
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    transport::send_message(&mut writer, &Message::Ready).await?;
    tracing::info!("sent ready signal, waiting for job");

    let job = match transport::recv_message(&mut reader).await? {
        Some(Message::JobDefinition(job)) => job,
        Some(other) => anyhow::bail!("expected JobDefinition, got {:?}", other.message_type()),
        None => anyhow::bail!("connection closed before receiving job"),
    };

    tracing::info!(job_id = %job.job_id, cmd = ?job.command, "received job");

    // If the host wired up networking, seed /etc/resolv.conf with the DNS
    // servers it picked. Silent failure is fine — jobs that don't use DNS
    // (e.g. vsock-only payloads) won't care.
    if let Some(dns) = job.environment.get("HOLLOW_DNS")
        && let Err(e) = write_resolv_conf(dns).await
    {
        tracing::warn!(error = %e, "failed to write /etc/resolv.conf");
    }

    // Prepare working directory and write files
    tokio::fs::create_dir_all(WORK_DIR).await?;
    for file in &job.files {
        let path = Path::new(WORK_DIR).join(&file.path);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, &file.content).await?;
        if file.mode != 0 {
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(file.mode)).await?;
        }
    }

    // Execute the job, streaming logs through a channel
    let (log_tx, log_rx) = mpsc::unbounded_channel::<Message>();

    // Spawn a task that drains log messages to the vsock writer
    let forward_handle = tokio::spawn(async move {
        let mut rx = log_rx;
        while let Some(msg) = rx.recv().await {
            if let Err(e) = transport::send_message(&mut writer, &msg).await {
                tracing::warn!(error = %e, "failed to forward log to host, stopping");
                break;
            }
        }
        writer
    });

    let result = execute_job(&job, log_tx).await;

    let mut writer = match forward_handle.await {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = %e, "log forwarder task panicked");
            return Err(anyhow::anyhow!("log forwarder panicked: {e}"));
        }
    };

    let (exit_code, plan_output) = match &result {
        Ok(output) => (0, output.clone()),
        Err(e) => {
            // Surface the failure detail back to the host so the resulting
            // CompleteRelease has more than just "exit code -1". One line
            // per cause (anyhow's chain); marked stderr so it routes to the
            // right channel in the controller's PushLogs.
            for line in format!("hollow-guest: {e:#}").lines() {
                let _ = transport::send_message(
                    &mut writer,
                    &Message::LogLine(LogLineMsg {
                        channel: "stderr".to_string(),
                        line: line.to_string(),
                        timestamp: now_millis(),
                    }),
                )
                .await;
            }
            (-1, None)
        }
    };

    transport::send_message(
        &mut writer,
        &Message::Completion(CompletionMsg {
            exit_code,
            plan_output,
        }),
    )
    .await?;

    tracing::info!(exit_code, "job completed");
    result.map(|_| ())
}

async fn execute_job(
    job: &JobDefinition,
    log_tx: mpsc::UnboundedSender<Message>,
) -> anyhow::Result<Option<String>> {
    if job.command.is_empty() {
        anyhow::bail!("empty command");
    }

    let mut cmd = tokio::process::Command::new(&job.command[0]);
    cmd.args(&job.command[1..])
        .current_dir(WORK_DIR)
        // Seed a sensible baseline before the job-supplied env takes over.
        // hollow-guest runs as PID 1 with an essentially empty environment —
        // Linux doesn't set PATH/HOME/TERM for init, systemd normally would,
        // but we have no systemd. Without this any `sh -c 'tofu …'` dies
        // with "not found" because PATH is empty.
        .env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .env("HOME", "/root")
        .env("TERM", "xterm-256color")
        // Apply the job's env last so callers can override any of the above.
        .envs(&job.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut proc = cmd.spawn().context("failed to spawn process")?;

    let is_plan = job.mode == "plan";

    let stdout = proc.stdout.take().context("missing stdout pipe")?;
    let stderr = proc.stderr.take().context("missing stderr pipe")?;

    let stdout_tx = log_tx.clone();
    let stdout_handle = tokio::spawn(stream_lines(
        stdout,
        "stdout".to_string(),
        stdout_tx,
        is_plan,
    ));

    let stderr_tx = log_tx.clone();
    let stderr_handle = tokio::spawn(stream_lines_discard(
        stderr,
        "stderr".to_string(),
        stderr_tx,
    ));

    // Drop our sender so the forward task sees channel close after stdout/stderr finish
    drop(log_tx);

    let exit = proc.wait().await.context("failed to wait for process")?;

    let captured_stdout = match stdout_handle.await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "stdout reader task failed");
            String::new()
        }
    };

    if let Err(e) = stderr_handle.await {
        tracing::warn!(error = %e, "stderr reader task failed");
    }

    if !exit.success() {
        anyhow::bail!("process exited with code {}", exit.code().unwrap_or(-1));
    }

    if is_plan {
        Ok(Some(captured_stdout))
    } else {
        Ok(None)
    }
}

/// Stream lines from a reader, sending each as a log message. Captures output
/// when `capture` is true and returns the captured text.
async fn stream_lines(
    reader: ChildStdout,
    channel: String,
    tx: mpsc::UnboundedSender<Message>,
    capture: bool,
) -> String {
    let mut lines = BufReader::new(reader).lines();
    let mut captured = String::new();
    while let Ok(Some(line)) = lines.next_line().await {
        let _ = tx.send(Message::LogLine(LogLineMsg {
            channel: channel.clone(),
            line: line.clone(),
            timestamp: now_millis(),
        }));
        if capture {
            if !captured.is_empty() {
                captured.push('\n');
            }
            captured.push_str(&line);
        }
    }
    captured
}

/// Stream lines from a reader, sending each as a log message. Does not capture.
async fn stream_lines_discard(
    reader: tokio::process::ChildStderr,
    channel: String,
    tx: mpsc::UnboundedSender<Message>,
) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let _ = tx.send(Message::LogLine(LogLineMsg {
            channel: channel.clone(),
            line,
            timestamp: now_millis(),
        }));
    }
}

async fn write_resolv_conf(dns_csv: &str) -> anyhow::Result<()> {
    let mut body = String::new();
    for server in dns_csv.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        body.push_str("nameserver ");
        body.push_str(server);
        body.push('\n');
    }
    if body.is_empty() {
        return Ok(());
    }
    // /etc lives on the read-only rootfs, so we can't overwrite the file
    // directly. /run is a tmpfs (mounted in setup_writable_areas); write
    // there and bind-mount it on top of /etc/resolv.conf so any tool that
    // reads the canonical path gets the writable copy.
    tokio::fs::write("/run/resolv.conf", body).await?;
    if let Err(e) = bind_mount("/run/resolv.conf", "/etc/resolv.conf") {
        // EBUSY just means we already bind-mounted on a previous job — that
        // can't happen with a fresh VM but we treat it as benign just in case.
        tracing::warn!(error = %e, "bind-mount /run/resolv.conf → /etc/resolv.conf failed");
    }
    Ok(())
}

fn bind_mount(source: &str, target: &str) -> anyhow::Result<()> {
    use std::ffi::CString;
    let src = CString::new(source)?;
    let tgt = CString::new(target)?;
    let rc = unsafe {
        libc::mount(
            src.as_ptr(),
            tgt.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND,
            std::ptr::null(),
        )
    };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        anyhow::bail!("bind-mount {source} → {target}: {err}");
    }
    Ok(())
}

/// Mount tmpfs at every directory the runtime needs to write to. The rootfs
/// is read-only, so without these mounts /work, /tmp, /run etc. would all
/// EROFS the moment a job tries to use them.
///
/// Idempotent: silently no-ops if the path is already a mount point (the
/// kernel might have set up `/dev` or `/proc` for us depending on the kernel
/// config). Errors on individual mounts are logged but don't abort the guest
/// — the worst case is the job fails with a clear "permission denied" later,
/// which beats hanging.
fn setup_writable_areas() -> anyhow::Result<()> {
    // size=… is generous but bounded by the VM's `mem_size_mib`, so a
    // runaway tmpfs can't exceed the VM's RAM cap.
    for (path, size_mib) in [
        ("/work", 512),
        ("/tmp", 512),
        ("/run", 64),
        ("/var/tmp", 256),
    ] {
        // Best-effort: ensure mount target exists. On a read-only rootfs we
        // rely on the image already having these dirs.
        let _ = std::fs::create_dir_all(path);
        if let Err(e) = mount_tmpfs(path, size_mib) {
            tracing::warn!(target = path, error = %e, "tmpfs mount failed");
        }
    }
    Ok(())
}

fn mount_tmpfs(target: &str, size_mib: u32) -> anyhow::Result<()> {
    use std::ffi::CString;
    let target_c = CString::new(target)?;
    let fstype_c = CString::new("tmpfs")?;
    let opts = format!("size={size_mib}M,mode=1777");
    let opts_c = CString::new(opts)?;
    // mount(source, target, fstype, mountflags, data) — for tmpfs the source
    // string is conventionally "tmpfs" but the kernel ignores it.
    let source_c = CString::new("tmpfs")?;
    let rc = unsafe {
        libc::mount(
            source_c.as_ptr(),
            target_c.as_ptr(),
            fstype_c.as_ptr(),
            0,
            opts_c.as_ptr().cast(),
        )
    };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        anyhow::bail!("mount tmpfs at {target}: {err}");
    }
    Ok(())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Connect to the host agent over AF_VSOCK.
///
/// The host (Firecracker side) is always CID=2. The port is fixed by convention
/// (`DEFAULT_PORT`); override with `HOLLOW_VSOCK_PORT` for diagnostics.
///
/// Boot ordering is racy: the guest kernel often starts before the host has
/// finished configuring the vsock device, so we retry until `CONNECT_TIMEOUT`.
async fn connect() -> anyhow::Result<VsockStream> {
    let port: u32 = std::env::var("HOLLOW_VSOCK_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    let addr = VsockAddr::new(HOST_CID, port);

    tracing::info!(cid = HOST_CID, port, "connecting via AF_VSOCK");
    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    let mut last_err: Option<std::io::Error> = None;
    while tokio::time::Instant::now() < deadline {
        match VsockStream::connect(addr).await {
            Ok(s) => return Ok(s),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(CONNECT_RETRY).await;
            }
        }
    }
    Err(last_err
        .map(|e| anyhow::anyhow!("vsock connect timed out: {e}"))
        .unwrap_or_else(|| anyhow::anyhow!("vsock connect timed out")))
        .with_context(|| format!("CID={HOST_CID}, port={port}"))
}
