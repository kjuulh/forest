//! Framed read/write over any AsyncRead/AsyncWrite (vsock, TCP, Unix socket).
//!
//! Frame format:
//! ```text
//! ┌──────────┬──────────┬─────────────┐
//! │ type(u8) │ len(u32) │ payload     │
//! └──────────┴──────────┴─────────────┘
//! ```
//! len is big-endian and does NOT include the 5-byte header.

use anyhow::{Context, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::protocol::{CompletionMsg, JobDefinition, LogLineMsg, Message, MessageType};

/// Maximum payload size (16 MiB — generous for file transfers).
const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;

/// Send a typed message over the stream.
pub async fn send_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    msg: &Message,
) -> anyhow::Result<()> {
    let msg_type = msg.message_type() as u8;
    let payload = match msg {
        Message::JobDefinition(v) => serde_json::to_vec(v)?,
        Message::LogLine(v) => serde_json::to_vec(v)?,
        Message::Completion(v) => serde_json::to_vec(v)?,
        Message::Ready | Message::Heartbeat | Message::Cancel => Vec::new(),
    };

    let len = payload.len() as u32;
    if len > MAX_PAYLOAD {
        bail!("payload too large: {len} bytes (max {MAX_PAYLOAD})");
    }

    writer.write_u8(msg_type).await?;
    writer.write_u32(len).await?;
    if !payload.is_empty() {
        writer.write_all(&payload).await?;
    }
    writer.flush().await?;
    Ok(())
}

/// Receive a typed message from the stream. Returns None on clean EOF.
pub async fn recv_message<R: AsyncRead + Unpin>(reader: &mut R) -> anyhow::Result<Option<Message>> {
    let msg_type_byte = match read_u8_or_eof(reader).await? {
        Some(b) => b,
        None => return Ok(None),
    };

    let msg_type = MessageType::from_u8(msg_type_byte)
        .with_context(|| format!("unknown message type: 0x{msg_type_byte:02x}"))?;

    let len = reader.read_u32().await.context("failed to read length")?;
    if len > MAX_PAYLOAD {
        bail!("payload too large: {len} bytes (max {MAX_PAYLOAD})");
    }

    let payload = if len > 0 {
        let mut buf = vec![0u8; len as usize];
        reader
            .read_exact(&mut buf)
            .await
            .context("failed to read payload")?;
        buf
    } else {
        Vec::new()
    };

    let msg = match msg_type {
        MessageType::JobDefinition => {
            let v: JobDefinition = serde_json::from_slice(&payload)?;
            Message::JobDefinition(v)
        }
        MessageType::Ready => Message::Ready,
        MessageType::LogLine => {
            let v: LogLineMsg = serde_json::from_slice(&payload)?;
            Message::LogLine(v)
        }
        MessageType::Heartbeat => Message::Heartbeat,
        MessageType::Completion => {
            let v: CompletionMsg = serde_json::from_slice(&payload)?;
            Message::Completion(v)
        }
        MessageType::Cancel => Message::Cancel,
    };

    Ok(Some(msg))
}

async fn read_u8_or_eof<R: AsyncRead + Unpin>(reader: &mut R) -> anyhow::Result<Option<u8>> {
    let mut buf = [0u8; 1];
    match reader.read(&mut buf).await? {
        0 => Ok(None),
        1 => Ok(Some(buf[0])),
        _ => unreachable!(),
    }
}
