use std::{fmt::Display, time::UNIX_EPOCH};

use serde::Serialize;
use sqlx::PgPool;
use tokio::sync::mpsc::UnboundedSender;

use crate::services::release_registry::ReleaseItem;

#[derive(Clone)]
pub struct DestinationLogger {
    input: UnboundedSender<(LogChannel, String)>,
}

impl DestinationLogger {
    pub fn new(release: ReleaseItem, db: PgPool) -> Self {
        let (input, mut output) = tokio::sync::mpsc::unbounded_channel();

        let _release = release.clone();
        let _handle = tokio::spawn(async move {
            let attempt = uuid::Uuid::now_v7();

            let db = db.clone();
            let release = _release.clone();

            let max_size = 1024 * 1000; // 1MB

            let mut buffer = Vec::new();
            let mut current_size = 0;
            let mut sequence = 0;
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            let commit_block = async move |sequence: &mut usize,
                                           buf: &mut Vec<LogLine>,
                                           current_size: &mut usize|
                        -> anyhow::Result<()> {
                tracing::trace!("committing log block: {}", sequence);

                sqlx::query!(
                    r#"
                        INSERT INTO release_logs (
                            release_attempt,
                            release_id,
                            destination_id,
                            log_lines,
                            sequence
                        ) VALUES (
                            $1,
                            $2,
                            $3,
                            $4,
                            $5
                        );
                    "#,
                    attempt,
                    release.id,
                    release.destination_id,
                    serde_json::to_value(&buf)?,
                    *sequence as i64
                )
                .execute(&db)
                .await?;

                // Reset
                buf.clear();
                *current_size = 0;
                *sequence += 1;

                Ok(())
            };

            loop {
                let msg: Option<(LogChannel, String)> = tokio::select! {
                    _ = interval.tick() => {
                        if current_size != 0 &&  let Err(e) = commit_block(&mut sequence, &mut buffer, &mut current_size).await {
                            tracing::error!("failed to commit log block: {:?}", e);
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

                        if current_size > max_size
                            && let Err(e) =
                                commit_block(&mut sequence, &mut buffer, &mut current_size).await
                        {
                            tracing::error!("failed to commit log block: {:?}", e);
                        }

                        tracing::debug!("pushing log message");
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
                        tracing::debug!("exiting destination logger");
                        if current_size != 0
                            && let Err(e) =
                                commit_block(&mut sequence, &mut buffer, &mut current_size).await
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
#[derive(Serialize, Debug, Clone)]
struct LogLine {
    channel: LogChannel,
    line: String,
    timestamp: u128,
}

#[derive(Serialize, Debug, Clone)]
enum LogChannel {
    #[serde(rename = "stdout")]
    Stdout,
    #[serde(rename = "stderr")]
    Stderr,
}

impl Display for LogChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogChannel::Stdout => f.write_str("stdout"),
            LogChannel::Stderr => f.write_str("stderr"),
        }
    }
}

impl LogChannel {
    pub fn size(&self) -> usize {
        self.to_string().len()
    }
}
