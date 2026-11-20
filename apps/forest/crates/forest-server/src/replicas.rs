use std::{sync::OnceLock, time::Duration};

use anyhow::Context;
use notmad::{Component, ComponentInfo, MadError};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::release_drain::ReleaseDrain;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    Silent {
        last_seen: chrono::DateTime<chrono::Utc>,
    },
    Stopped,
    Gone,
}

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
pub const ALIVE_WINDOW: Duration = Duration::from_secs(4 * 30);
pub const DEFAULT_GONE_AFTER: Duration = Duration::from_secs(20 * 60);
pub const MIN_GONE_AFTER: Duration = ALIVE_WINDOW;
const FORGET_AFTER_SECS: f64 = 24.0 * 60.0 * 60.0;

pub fn replica_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        let host = host_name();
        let instance = uuid::Uuid::new_v4().simple().to_string();
        format!("{}#{}", sanitise(&host), &instance[..8])
    })
}

fn host_name() -> String {
    for var in ["FOREST_REPLICA_NAME", "HOSTNAME"] {
        if let Ok(value) = std::env::var(var)
            && !value.trim().is_empty()
        {
            return value.trim().to_string();
        }
    }
    let mut buf = [0u8; 256];
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc == 0 {
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        if let Ok(name) = std::str::from_utf8(&buf[..end])
            && !name.is_empty()
        {
            return name.to_string();
        }
    }
    "unknown-replica".to_string()
}

fn sanitise(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            ':' | '@' | '#' | '/' => '-',
            c if c.is_whitespace() => '-',
            c => c,
        })
        .collect()
}

pub async fn beat(db: &PgPool, name: &str) -> anyhow::Result<()> {
    sqlx::query!(
        r#"
            INSERT INTO forest_replicas (name, started_at, last_seen_at)
            VALUES ($1, now(), now())
            ON CONFLICT (name) DO UPDATE SET last_seen_at = now()
        "#,
        name,
    )
    .execute(db)
    .await
    .context("record replica heartbeat")?;

    sqlx::query!(
        r#"
            DELETE FROM forest_replicas
            WHERE last_seen_at < now() - make_interval(secs => $1::double precision)
        "#,
        FORGET_AFTER_SECS,
    )
    .execute(db)
    .await
    .context("forget long-gone replicas")?;
    Ok(())
}

pub async fn liveness(db: &PgPool, name: &str, gone_after: Duration) -> anyhow::Result<Liveness> {
    let row = sqlx::query!(
        r#"
            SELECT
                last_seen_at,
                stopped_at,
                last_seen_at > now() - make_interval(secs => $2::double precision) AS "alive!",
                last_seen_at > now() - make_interval(secs => $3::double precision) AS "heard!"
            FROM forest_replicas
            WHERE name = $1
        "#,
        name,
        ALIVE_WINDOW.as_secs_f64(),
        gone_after.as_secs_f64(),
    )
    .fetch_optional(db)
    .await
    .context("check replica liveness")?;

    Ok(match row {
        None => Liveness::Gone,
        Some(r) if r.stopped_at.is_some() => Liveness::Stopped,
        Some(r) if r.alive => Liveness::Alive,
        Some(r) if r.heard => Liveness::Silent {
            last_seen: r.last_seen_at,
        },
        Some(_) => Liveness::Gone,
    })
}

pub async fn deregister(db: &PgPool, name: &str) -> anyhow::Result<()> {
    sqlx::query!(
        r#"
            UPDATE forest_replicas
            SET stopped_at = now(), last_seen_at = now()
            WHERE name = $1
        "#,
        name,
    )
    .execute(db)
    .await
    .context("mark replica stopped")?;
    Ok(())
}

pub struct ReplicaHeartbeat {
    db: PgPool,
    drain: ReleaseDrain,
    name: String,
}

impl ReplicaHeartbeat {
    pub fn new(db: PgPool, drain: ReleaseDrain) -> Self {
        Self::named(db, drain, replica_name().to_string())
    }

    pub fn named(db: PgPool, drain: ReleaseDrain, name: String) -> Self {
        Self { db, drain, name }
    }

    async fn stop(&self) {
        if !self.drain.stopped_every_release() {
            tracing::warn!(
                replica = %self.name,
                "release tasks were still running at shutdown; not marking this replica stopped, \
                 so its terraform state locks are only cleared once it has been silent for \
                 FOREST_REPLICA_GONE_SECS"
            );
            return;
        }
        match deregister(&self.db, &self.name).await {
            Ok(()) => tracing::info!(replica = %self.name, "replica marked stopped"),
            Err(e) => tracing::warn!(replica = %self.name, "failed to mark replica stopped: {e:#}"),
        }
    }
}

impl Component for ReplicaHeartbeat {
    fn info(&self) -> ComponentInfo {
        "forest-server/replica-heartbeat".into()
    }

    async fn setup(&self) -> Result<(), MadError> {
        tracing::info!(replica = %self.name, "replica heartbeat starting");
        if let Err(e) = beat(&self.db, &self.name).await {
            tracing::warn!("first replica heartbeat failed: {e:#}");
        }
        Ok(())
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(e) = beat(&self.db, &self.name).await {
                        tracing::warn!("replica heartbeat failed: {e:#}");
                    }
                }
            }
        }
        self.stop().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_process_gets_its_own_replica_name_that_survives_a_principal() {
        let name = replica_name();
        assert_eq!(name, replica_name(), "stable within a process");
        let (host, instance) = name.rsplit_once('#').expect("host#instance");
        assert!(!host.is_empty());
        assert_eq!(instance.len(), 8);
        assert!(!name.contains(':') && !name.contains('@'));
    }

    #[test]
    fn separators_in_a_configured_name_are_replaced() {
        assert_eq!(sanitise("a:b@c#d/e f"), "a-b-c-d-e-f");
    }
}
