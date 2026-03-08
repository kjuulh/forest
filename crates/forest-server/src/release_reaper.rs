use std::time::Duration;

use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::{
    State,
    runner_manager::RunnerManager,
    services::release_event_store::{
        EventPayload, ReleaseEventStore, ReleaseEventStoreState, ReleaseEventType,
    },
};

pub struct ReleaseReaper {
    release_event_store: ReleaseEventStore,
    runner_manager: RunnerManager,
    assigned_timeout: Duration,
    running_timeout: Duration,
    heartbeat_timeout: Duration,
}

impl ReleaseReaper {
    pub fn new(state: &State, runner_manager: RunnerManager) -> Self {
        Self {
            release_event_store: state.release_event_store(),
            runner_manager,
            assigned_timeout: Duration::from_secs(5 * 60),
            running_timeout: Duration::from_secs(60 * 60),
            // 3 missed heartbeats (30s each) = 90s threshold
            heartbeat_timeout: Duration::from_secs(90),
        }
    }

    async fn reap(&self) -> anyhow::Result<()> {
        // Check for stale heartbeats first (faster detection)
        let stale = self
            .release_event_store
            .find_stale_heartbeats(self.heartbeat_timeout.as_secs() as i64)
            .await?;

        for release in stale {
            let msg = format!(
                "release heartbeat lost (was {} with no heartbeat for >{}s)",
                release.status,
                self.heartbeat_timeout.as_secs()
            );
            tracing::warn!(
                release_id = %release.release_id,
                status = %release.status,
                runner_id = ?release.runner_id,
                "reaping release with stale heartbeat"
            );

            if let Err(e) = self
                .release_event_store
                .emit_event(
                    release.release_id,
                    ReleaseEventType::Failed,
                    EventPayload {
                        error_message: Some(msg),
                        ..Default::default()
                    },
                    None,
                )
                .await
            {
                tracing::debug!(
                    release_id = %release.release_id,
                    "failed to reap stale release (likely already transitioned): {e}"
                );
            }
        }

        // Fallback: catch releases that predate the heartbeat column (last_heartbeat_at IS NULL)
        let stuck = self
            .release_event_store
            .find_stuck_releases(
                self.assigned_timeout.as_secs() as i64,
                self.running_timeout.as_secs() as i64,
            )
            .await?;

        for release in stuck {
            let msg = format!(
                "release timed out (was {} for too long)",
                release.status
            );
            tracing::warn!(
                release_id = %release.release_id,
                status = %release.status,
                "reaping stuck release"
            );

            if let Err(e) = self
                .release_event_store
                .emit_event(
                    release.release_id,
                    ReleaseEventType::TimedOut,
                    EventPayload {
                        error_message: Some(msg),
                        ..Default::default()
                    },
                    None,
                )
                .await
            {
                tracing::debug!(
                    release_id = %release.release_id,
                    "failed to reap release (likely already transitioned): {e}"
                );
            }
        }

        self.runner_manager.reap_stale(self.assigned_timeout).await;

        Ok(())
    }
}

impl Component for ReleaseReaper {
    fn info(&self) -> ComponentInfo {
        "forest-server/release-reaper".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(e) = self.reap().await {
                        tracing::error!("release reaper error: {e:#}");
                    }
                }
            }
        }

        Ok(())
    }
}
