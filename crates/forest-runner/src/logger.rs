use std::time::{SystemTime, UNIX_EPOCH};

use forest_grpc_interface::PushLogRequest;
use tokio::sync::mpsc;

/// Logger that streams log lines to the server via gRPC PushLogs.
///
/// Log lines are sent through a channel to a background task that
/// streams them to the server. Drop the logger to flush and close.
#[derive(Clone)]
pub struct RemoteLogger {
    release_token: String,
    sender: mpsc::UnboundedSender<PushLogRequest>,
}

impl RemoteLogger {
    pub fn new(release_token: String, sender: mpsc::UnboundedSender<PushLogRequest>) -> Self {
        Self {
            release_token,
            sender,
        }
    }

    pub fn log_stdout(&self, line: &str) {
        self.send("stdout", line);
    }

    pub fn log_stderr(&self, line: &str) {
        self.send("stderr", line);
    }

    fn send(&self, channel: &str, line: &str) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let _ = self.sender.send(PushLogRequest {
            release_token: self.release_token.clone(),
            channel: channel.to_string(),
            line: line.to_string(),
            timestamp,
        });
    }
}
