//! VM lifecycle management — creates and manages Firecracker microVMs for jobs.
//!
//! For Phase 1 development, this module uses a Unix socket + subprocess to simulate
//! the VM. Production will use Firecracker API + vsock.

use std::process::Stdio;
use std::time::Duration;

use hollow_grpc_interface::{
    AgentMessage, JobLogBatch, JobStatus, JobUpdate, LogLine, RunJob, agent_message,
};
use hollow_vsock::protocol::{JobDefinition, JobFile, Message};
use hollow_vsock::transport;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Run a job. In production this launches a Firecracker VM; in dev mode it spawns
/// the hollow-guest binary directly, communicating over a Unix socket.
pub async fn run_job(
    job: RunJob,
    outbound_tx: mpsc::UnboundedSender<AgentMessage>,
    data_dir: &str,
    cancel: CancellationToken,
) {
    let job_id = job.job_id.clone();

    // Report booting
    send_status(&outbound_tx, &job_id, JobStatus::Booting, None);

    let result = tokio::select! {
        r = run_job_inner(job, &outbound_tx, data_dir) => r,
        _ = cancel.cancelled() => {
            tracing::info!(job_id = %job_id, "job cancelled");
            Err(anyhow::anyhow!("job cancelled"))
        }
    };

    match result {
        Ok((exit_code, plan_output)) => {
            let status = if exit_code == 0 {
                JobStatus::Completed
            } else {
                JobStatus::Failed
            };
            let _ = outbound_tx.send(AgentMessage {
                message: Some(agent_message::Message::JobUpdate(JobUpdate {
                    job_id,
                    status: status.into(),
                    error_message: if exit_code != 0 {
                        format!("process exited with code {exit_code}")
                    } else {
                        String::new()
                    },
                    plan_output,
                    exit_code,
                })),
            });
        }
        Err(e) => {
            tracing::error!(job_id = %job_id, error = %e, "job failed");
            send_status(
                &outbound_tx,
                &job_id,
                JobStatus::Failed,
                Some(format!("{e:#}")),
            );
        }
    }
}

async fn run_job_inner(
    job: RunJob,
    outbound_tx: &mpsc::UnboundedSender<AgentMessage>,
    data_dir: &str,
) -> anyhow::Result<(i32, Option<String>)> {
    let job_id = &job.job_id;
    let socket_path = format!("{data_dir}/vm-{job_id}.sock");

    // Ensure data_dir exists
    tokio::fs::create_dir_all(data_dir).await?;

    // Clean up stale socket
    let _ = tokio::fs::remove_file(&socket_path).await;

    // Listen for the guest to connect
    let listener = tokio::net::UnixListener::bind(&socket_path)?;

    // In dev mode, auto-launch hollow-guest as a subprocess.
    // In production, this will be replaced by Firecracker VM launch.
    let guest_bin =
        std::env::var("HOLLOW_GUEST_BIN").unwrap_or_else(|_| "hollow-guest".to_string());
    tracing::info!(job_id, socket = %socket_path, guest_bin = %guest_bin, "launching guest");

    let mut guest_proc = tokio::process::Command::new(&guest_bin)
        .env("HOLLOW_VSOCK_PATH", &socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn guest binary '{guest_bin}': {e}"))?;

    send_status(outbound_tx, job_id, JobStatus::Running, None);

    // Accept guest connection (with timeout)
    let stream = tokio::time::timeout(Duration::from_secs(30), listener.accept()).await;
    let (stream, _) = match stream {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            guest_proc.kill().await.ok();
            anyhow::bail!("accept failed: {e}");
        }
        Err(_) => {
            guest_proc.kill().await.ok();
            anyhow::bail!("guest did not connect within 30s");
        }
    };

    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(reader);

    // Wait for Ready signal
    match transport::recv_message(&mut reader).await? {
        Some(Message::Ready) => tracing::info!(job_id, "guest ready"),
        other => anyhow::bail!("expected Ready, got {other:?}"),
    }

    // Send job definition
    let job_def = JobDefinition {
        job_id: job.job_id.clone(),
        command: job.command.clone(),
        environment: job.environment.clone(),
        files: job
            .files
            .iter()
            .map(|f| JobFile {
                path: f.path.clone(),
                content: f.content.clone(),
                mode: f.mode,
            })
            .collect(),
        mode: job.mode.clone(),
        timeout_seconds: job.timeout_seconds,
    };

    transport::send_message(&mut writer, &Message::JobDefinition(job_def)).await?;
    tracing::info!(job_id, "sent job definition");

    // Read messages from guest until completion
    let timeout = Duration::from_secs(job.timeout_seconds.max(60) as u64);
    let deadline = tokio::time::Instant::now() + timeout;

    let result = run_guest_loop(&job.job_id, outbound_tx, &mut reader, deadline).await;

    // Cleanup: kill guest process and remove socket on all paths.
    // guest_proc has kill_on_drop(true), but explicit kill is more reliable.
    guest_proc.kill().await.ok();
    guest_proc.wait().await.ok();
    let _ = tokio::fs::remove_file(&socket_path).await;

    result
}

async fn run_guest_loop(
    job_id: &str,
    outbound_tx: &mpsc::UnboundedSender<AgentMessage>,
    reader: &mut tokio::io::BufReader<tokio::io::ReadHalf<tokio::net::UnixStream>>,
    deadline: tokio::time::Instant,
) -> anyhow::Result<(i32, Option<String>)> {
    loop {
        let msg = tokio::time::timeout_at(deadline, transport::recv_message(reader)).await;

        match msg {
            Ok(Ok(Some(Message::LogLine(log)))) => {
                let _ = outbound_tx.send(AgentMessage {
                    message: Some(agent_message::Message::LogBatch(JobLogBatch {
                        job_id: job_id.to_string(),
                        lines: vec![LogLine {
                            channel: log.channel,
                            line: log.line,
                            timestamp: log.timestamp,
                        }],
                    })),
                });
            }
            Ok(Ok(Some(Message::Heartbeat))) => {}
            Ok(Ok(Some(Message::Completion(c)))) => {
                tracing::info!(job_id, exit_code = c.exit_code, "guest reported completion");
                return Ok((c.exit_code, c.plan_output));
            }
            Ok(Ok(Some(other))) => {
                tracing::warn!(job_id, msg_type = ?other.message_type(), "unexpected message from guest");
            }
            Ok(Ok(None)) => anyhow::bail!("guest disconnected"),
            Ok(Err(e)) => anyhow::bail!("vsock read error: {e}"),
            Err(_) => {
                anyhow::bail!(
                    "job timed out after {}s",
                    deadline
                        .duration_since(tokio::time::Instant::now())
                        .as_secs()
                );
            }
        }
    }
}

fn send_status(
    tx: &mpsc::UnboundedSender<AgentMessage>,
    job_id: &str,
    status: JobStatus,
    error: Option<String>,
) {
    let _ = tx.send(AgentMessage {
        message: Some(agent_message::Message::JobUpdate(JobUpdate {
            job_id: job_id.to_string(),
            status: status.into(),
            error_message: error.unwrap_or_default(),
            plan_output: None,
            exit_code: 0,
        })),
    });
}
