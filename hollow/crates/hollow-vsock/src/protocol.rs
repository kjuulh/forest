//! Message types exchanged between host agent and guest over vsock.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Wire message types.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    /// Host → Guest: job definition with command, env, files.
    JobDefinition = 0x01,
    /// Guest → Host: guest agent is ready to receive a job.
    Ready = 0x02,
    /// Guest → Host: a log line from the running process.
    LogLine = 0x03,
    /// Guest → Host: periodic heartbeat.
    Heartbeat = 0x04,
    /// Guest → Host: job completed with exit code.
    Completion = 0x05,
    /// Host → Guest: cancel the running job.
    Cancel = 0x06,
}

impl MessageType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(Self::JobDefinition),
            0x02 => Some(Self::Ready),
            0x03 => Some(Self::LogLine),
            0x04 => Some(Self::Heartbeat),
            0x05 => Some(Self::Completion),
            0x06 => Some(Self::Cancel),
            _ => None,
        }
    }
}

/// Host → Guest: full job specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobDefinition {
    pub job_id: String,
    pub command: Vec<String>,
    pub environment: HashMap<String, String>,
    pub files: Vec<JobFile>,
    pub mode: String,
    pub timeout_seconds: u32,
}

/// A file to write into the guest working directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobFile {
    pub path: String,
    #[serde(with = "base64_bytes")]
    pub content: Vec<u8>,
    pub mode: u32,
}

/// Guest → Host: a log line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLineMsg {
    pub channel: String,
    pub line: String,
    pub timestamp: u64,
}

/// Guest → Host: job completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionMsg {
    pub exit_code: i32,
    pub plan_output: Option<String>,
}

/// Typed message envelope for dispatch.
#[derive(Debug, Clone)]
pub enum Message {
    JobDefinition(JobDefinition),
    Ready,
    LogLine(LogLineMsg),
    Heartbeat,
    Completion(CompletionMsg),
    Cancel,
}

impl Message {
    pub fn message_type(&self) -> MessageType {
        match self {
            Self::JobDefinition(_) => MessageType::JobDefinition,
            Self::Ready => MessageType::Ready,
            Self::LogLine(_) => MessageType::LogLine,
            Self::Heartbeat => MessageType::Heartbeat,
            Self::Completion(_) => MessageType::Completion,
            Self::Cancel => MessageType::Cancel,
        }
    }
}

/// Base64 serde helper for binary file content.
mod base64_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        use base64::Engine;
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        use base64::Engine;
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(&s)
            .map_err(serde::de::Error::custom)
    }
}
