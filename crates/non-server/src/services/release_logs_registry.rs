use std::fmt::Display;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::State;

#[derive(Clone)]
pub struct ReleaseLogsRegistry {
    db: PgPool,
}

impl ReleaseLogsRegistry {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Insert a batch of log lines for a release attempt
    pub async fn insert_log_block(
        &self,
        attempt: Uuid,
        release_id: Uuid,
        destination_id: Uuid,
        log_lines: &[LogLine],
        sequence: i64,
    ) -> anyhow::Result<()> {
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
            release_id,
            destination_id,
            serde_json::to_value(log_lines).context("serialize log lines")?,
            sequence
        )
        .execute(&self.db)
        .await
        .context("insert log block")?;

        Ok(())
    }

    /// Get all log blocks for a release attempt, ordered by sequence
    pub async fn get_logs_by_attempt(&self, attempt: Uuid) -> anyhow::Result<Vec<LogBlock>> {
        let records = sqlx::query!(
            r#"
                SELECT
                    id,
                    release_attempt,
                    release_id,
                    destination_id,
                    log_lines,
                    sequence,
                    created
                FROM release_logs
                WHERE release_attempt = $1
                ORDER BY sequence ASC
            "#,
            attempt
        )
        .fetch_all(&self.db)
        .await
        .context("get logs by attempt")?;

        records
            .into_iter()
            .map(|r| {
                Ok(LogBlock {
                    id: r.id,
                    release_attempt: r.release_attempt,
                    release_id: r.release_id,
                    destination_id: r.destination_id,
                    log_lines: serde_json::from_value(r.log_lines).context("parse log lines")?,
                    sequence: r.sequence,
                    created: r.created,
                })
            })
            .collect()
    }

    /// Get all log blocks for a release, ordered by sequence
    pub async fn get_logs_by_release(&self, release_id: Uuid) -> anyhow::Result<Vec<LogBlock>> {
        let records = sqlx::query!(
            r#"
                SELECT
                    id,
                    release_attempt,
                    release_id,
                    destination_id,
                    log_lines,
                    sequence,
                    created
                FROM release_logs
                WHERE release_id = $1
                ORDER BY created ASC, sequence ASC
            "#,
            release_id
        )
        .fetch_all(&self.db)
        .await
        .context("get logs by release")?;

        records
            .into_iter()
            .map(|r| {
                Ok(LogBlock {
                    id: r.id,
                    release_attempt: r.release_attempt,
                    release_id: r.release_id,
                    destination_id: r.destination_id,
                    log_lines: serde_json::from_value(r.log_lines).context("parse log lines")?,
                    sequence: r.sequence,
                    created: r.created,
                })
            })
            .collect()
    }

    /// Get log blocks for a release and destination after a given sequence (cursor-based)
    /// Returns blocks where sequence > after_sequence, ordered by sequence ASC
    pub async fn get_logs_after_sequence(
        &self,
        release_id: Uuid,
        destination_id: Uuid,
        after_sequence: i64,
    ) -> anyhow::Result<Vec<LogBlock>> {
        let records = sqlx::query!(
            r#"
                SELECT
                    id,
                    release_attempt,
                    release_id,
                    destination_id,
                    log_lines,
                    sequence,
                    created
                FROM release_logs
                WHERE release_id = $1
                  AND destination_id = $2
                  AND sequence > $3
                ORDER BY sequence ASC
            "#,
            release_id,
            destination_id,
            after_sequence
        )
        .fetch_all(&self.db)
        .await
        .context("get logs after sequence")?;

        records
            .into_iter()
            .map(|r| {
                Ok(LogBlock {
                    id: r.id,
                    release_attempt: r.release_attempt,
                    release_id: r.release_id,
                    destination_id: r.destination_id,
                    log_lines: serde_json::from_value(r.log_lines).context("parse log lines")?,
                    sequence: r.sequence,
                    created: r.created,
                })
            })
            .collect()
    }
}

pub trait ReleaseLogsRegistryState {
    fn release_logs_registry(&self) -> ReleaseLogsRegistry;
}

impl ReleaseLogsRegistryState for State {
    fn release_logs_registry(&self) -> ReleaseLogsRegistry {
        ReleaseLogsRegistry::new(self.db.clone())
    }
}

#[derive(Debug, Clone)]
pub struct LogBlock {
    pub id: Uuid,
    pub release_attempt: Uuid,
    pub release_id: Uuid,
    pub destination_id: Uuid,
    pub log_lines: Vec<LogLine>,
    pub sequence: i64,
    pub created: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LogLine {
    pub channel: LogChannel,
    pub line: String,
    pub timestamp: u128,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogChannel {
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
        match self {
            LogChannel::Stdout => 6,
            LogChannel::Stderr => 6,
        }
    }
}
