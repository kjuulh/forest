use std::time::UNIX_EPOCH;

use tokio::sync::mpsc::UnboundedSender;

use crate::services::{
    release_logs_registry::{LogChannel, LogLine, ReleaseLogsRegistry},
    release_registry::ReleaseItem,
};

#[derive(Clone)]
pub struct DestinationLogger {
    input: UnboundedSender<(LogChannel, String)>,
}

impl DestinationLogger {
    pub fn new(release: ReleaseItem, registry: ReleaseLogsRegistry) -> Self {
        let (input, mut output) = tokio::sync::mpsc::unbounded_channel();

        let _release = release.clone();
        let _handle = tokio::spawn(async move {
            let attempt = uuid::Uuid::now_v7();
            let release = _release.clone();

            let max_size = 1024 * 1000; // 1MB

            let mut buffer: Vec<LogLine> = Vec::new();
            let mut current_size = 0;
            let mut sequence: i64 = 0;
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));

            loop {
                let msg: Option<(LogChannel, String)> = tokio::select! {
                    _ = interval.tick() => {
                        if current_size != 0 {
                            if let Err(e) = registry.insert_log_block(
                                attempt,
                                release.id,
                                release.destination_id,
                                &buffer,
                                sequence,
                            ).await {
                                tracing::error!("failed to commit log block: {:?}", e);
                            } else {
                                buffer.clear();
                                current_size = 0;
                                sequence += 1;
                            }
                        }
                        continue;
                    }
                    msg = output.recv() => {
                       msg
                    }
                };

                match msg {
                    Some((channel, line)) => {
                        current_size += channel.size() + line.len();

                        if current_size > max_size {
                            if let Err(e) = registry
                                .insert_log_block(
                                    attempt,
                                    release.id,
                                    release.destination_id,
                                    &buffer,
                                    sequence,
                                )
                                .await
                            {
                                tracing::error!("failed to commit log block: {:?}", e);
                            } else {
                                buffer.clear();
                                current_size = 0;
                                sequence += 1;
                            }
                        }

                        buffer.push(LogLine {
                            channel,
                            line,
                            timestamp: std::time::SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .expect("to be able to get system time")
                                .as_millis(),
                        });
                    }
                    None => {
                        tracing::trace!("exiting destination logger");
                        if current_size != 0
                            && let Err(e) = registry
                                .insert_log_block(
                                    attempt,
                                    release.id,
                                    release.destination_id,
                                    &buffer,
                                    sequence,
                                )
                                .await
                        {
                            tracing::error!("failed to commit log block: {:?}", e);
                        }
                        return;
                    }
                }
            }
        });

        Self { input }
    }

    pub fn log_stdout(&self, line: &str) {
        self.log(LogChannel::Stdout, line);
    }

    pub fn log_stderr(&self, line: &str) {
        self.log(LogChannel::Stderr, line);
    }

    fn log(&self, channel: LogChannel, line: &str) {
        if let Err(e) = self.input.send((channel, line.to_string())) {
            tracing::warn!("failed to send log line: {:?}", e);
        }
    }
}
