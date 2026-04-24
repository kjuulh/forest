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

/// Working directory inside the VM for job files.
const WORK_DIR: &str = "/work";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("hollow-guest starting");

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
        Err(_) => (-1, None),
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

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Connect to host agent. Uses Unix socket for development, vsock in production.
async fn connect() -> anyhow::Result<tokio::net::UnixStream> {
    // TODO: Use AF_VSOCK on Linux in production (CID=2, port=1024).
    let path =
        std::env::var("HOLLOW_VSOCK_PATH").unwrap_or_else(|_| "/tmp/hollow-vsock.sock".to_string());

    tracing::info!(path = %path, "connecting via Unix socket");
    tokio::net::UnixStream::connect(&path)
        .await
        .with_context(|| format!("failed to connect to {path}"))
}
