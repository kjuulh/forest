use std::sync::Arc;
use std::time::Duration;

use forage_core::session::FileSessionStore;
use forage_db::PgSessionStore;
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

/// Session reaper for PostgreSQL-backed sessions.
pub struct PgSessionReaper {
    pub store: Arc<PgSessionStore>,
    pub max_inactive_days: i64,
}

impl Component for PgSessionReaper {
    fn info(&self) -> ComponentInfo {
        "forage/session-reaper-pg".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => break,
                _ = interval.tick() => {
                    match self.store.reap_expired(self.max_inactive_days).await {
                        Ok(n) if n > 0 => tracing::info!("session reaper: removed {n} expired sessions"),
                        Err(e) => tracing::warn!("session reaper error: {e}"),
                        _ => {}
                    }
                }
            }
        }

        Ok(())
    }
}

/// Session reaper for file-backed sessions.
pub struct FileSessionReaper {
    pub store: Arc<FileSessionStore>,
}

impl Component for FileSessionReaper {
    fn info(&self) -> ComponentInfo {
        "forage/session-reaper-file".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => break,
                _ = interval.tick() => {
                    self.store.reap_expired();
                    tracing::debug!("session reaper: {} active sessions", self.store.session_count());
                }
            }
        }

        Ok(())
    }
}
