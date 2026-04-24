//! Host side of the vsock job protocol.
//!
//! Firecracker bridges AF_VSOCK ↔ Unix domain socket. When the guest connects
//! to (CID=HOST=2, port=N), Firecracker accepts on `<vsock_uds>_<N>` on the
//! host and tunnels bytes both ways. So we just listen on a port-suffixed UDS
//! path and treat the resulting stream like any other framed connection.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use hollow_vsock::protocol::{JobDefinition, LogLineMsg, Message, MessageType};
use hollow_vsock::transport;
use tokio::net::{UnixListener, UnixStream};

/// vsock port the guest connects out to (must match `hollow-guest`'s `DEFAULT_PORT`).
pub const GUEST_TO_HOST_PORT: u32 = 1024;

pub struct GuestSession {
    listener: UnixListener,
    /// Kept so the suffixed UDS path is removable in `Drop`.
    uds_path: PathBuf,
}

impl GuestSession {
    /// Bind a Unix listener at `<vsock_uds_base>_<port>`. Must be called BEFORE
    /// the guest tries to connect — i.e. before Firecracker's `InstanceStart`.
    pub fn bind(vsock_uds_base: &Path, port: u32) -> anyhow::Result<Self> {
        let uds_path = port_suffixed_path(vsock_uds_base, port);
        let _ = std::fs::remove_file(&uds_path);
        let listener = UnixListener::bind(&uds_path)
            .with_context(|| format!("bind {}", uds_path.display()))?;
        Ok(Self { listener, uds_path })
    }

    /// Wait for the guest to dial in (with timeout). Returns the connected stream.
    pub async fn accept(&self, timeout: Duration) -> anyhow::Result<UnixStream> {
        let (stream, _) = tokio::time::timeout(timeout, self.listener.accept())
            .await
            .map_err(|_| anyhow::anyhow!("guest did not connect within {:?}", timeout))?
            .context("accept guest connection")?;
        Ok(stream)
    }
}

impl Drop for GuestSession {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.uds_path);
    }
}

fn port_suffixed_path(base: &Path, port: u32) -> PathBuf {
    let mut s = base.to_string_lossy().into_owned();
    s.push('_');
    s.push_str(&port.to_string());
    PathBuf::from(s)
}

/// Granular protocol events surfaced through the consumer's callback.
#[derive(Debug, Clone)]
pub enum JobEvent {
    JobDispatched,
    Log(LogLineMsg),
    Heartbeat,
    UnexpectedMessage(MessageType),
}

#[derive(Debug, Clone)]
pub struct JobOutcome {
    pub exit_code: i32,
    pub plan_output: Option<String>,
}

/// Drive one job to completion: handshake with the guest, send the job spec,
/// stream logs, capture completion. The caller receives every event via
/// `on_event` (to surface as JSONL, gRPC frames, etc.).
pub async fn drive_job<F>(
    stream: UnixStream,
    job: JobDefinition,
    mut on_event: F,
) -> anyhow::Result<JobOutcome>
where
    F: FnMut(JobEvent),
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = tokio::io::BufReader::new(reader);

    // Wait for the guest to declare readiness.
    match transport::recv_message(&mut reader)
        .await
        .context("recv from guest")?
    {
        Some(Message::Ready) => {}
        Some(other) => bail!("expected Ready from guest, got {:?}", other.message_type()),
        None => bail!("guest connection closed before Ready"),
    }

    transport::send_message(&mut writer, &Message::JobDefinition(job))
        .await
        .context("send JobDefinition")?;
    on_event(JobEvent::JobDispatched);

    loop {
        match transport::recv_message(&mut reader)
            .await
            .context("recv from guest")?
        {
            Some(Message::LogLine(l)) => on_event(JobEvent::Log(l)),
            Some(Message::Heartbeat) => on_event(JobEvent::Heartbeat),
            Some(Message::Completion(c)) => {
                return Ok(JobOutcome {
                    exit_code: c.exit_code,
                    plan_output: c.plan_output,
                });
            }
            Some(other) => on_event(JobEvent::UnexpectedMessage(other.message_type())),
            None => bail!("guest disconnected before completion"),
        }
    }
}
