use std::time::Duration;

use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use forest_models::ReleaseStatus;
use uuid::Uuid;

use crate::{
    State,
    runner_manager::RunnerManager,
    services::{
        destination_registry::{DestinationRegistry, DestinationRegistryState},
        notification_registry::{NotificationRegistry, NotificationRegistryState},
        release_event_store::{
            EventPayload, ReleaseEventStore, ReleaseEventStoreState, ReleaseEventType,
        },
        release_finalizer,
        release_registry::{ReleaseItem, ReleaseRegistry, ReleaseRegistryState},
    },
};

pub struct ReleaseReaper {
    release_event_store: ReleaseEventStore,
    release_registry: ReleaseRegistry,
    notification_registry: NotificationRegistry,
    destination_registry: DestinationRegistry,
    runner_manager: RunnerManager,
    assigned_timeout: Duration,
    running_timeout: Duration,
    heartbeat_timeout: Duration,
    intent_finalize_grace: Duration,
}

impl ReleaseReaper {
    pub fn new(state: &State, runner_manager: RunnerManager) -> Self {
        Self {
            release_event_store: state.release_event_store(),
            release_registry: state.release_registry(),
            notification_registry: state.notification_registry(),
            destination_registry: state.destination_registry(),
            runner_manager,
            assigned_timeout: Duration::from_secs(5 * 60),
            running_timeout: Duration::from_secs(60 * 60),
            heartbeat_timeout: Duration::from_secs(90),
            intent_finalize_grace: Duration::from_secs(2 * 60),
        }
    }

    /// A reaped release ends without its runner reporting back, so nothing
    /// else announces the failure: without this, Slack and webhooks keep
    /// showing it as running.
    async fn notify_failed(&self, release_id: Uuid, message: &str) {
        let state = match self
            .release_event_store
            .get_release_state(&release_id)
            .await
        {
            Ok(state) => state,
            Err(e) => {
                tracing::warn!(%release_id, "reaped release has no state to notify about: {e:#}");
                return;
            }
        };
        let item = ReleaseItem {
            id: release_id,
            release_intent_id: state.release_intent_id,
            artifact: state.artifact_id,
            project_id: state.project_id,
            destination_id: state.destination_id,
            status: state.status,
            project: String::new(),
        };
        if let Err(e) = release_finalizer::send_notification(
            &self.release_registry,
            &self.notification_registry,
            &self.destination_registry,
            &item,
            ReleaseStatus::Failed,
            Some(message),
        )
        .await
        {
            tracing::warn!(%release_id, "failed to notify about reaped release: {e:#}");
        }
    }

    pub async fn reap(&self) -> anyhow::Result<()> {
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

            match self
                .release_event_store
                .emit_event(
                    release.release_id,
                    ReleaseEventType::Failed,
                    EventPayload {
                        error_message: Some(msg.clone()),
                        ..Default::default()
                    },
                    None,
                )
                .await
            {
                Ok(_) => self.notify_failed(release.release_id, &msg).await,
                Err(e) => tracing::debug!(
                    release_id = %release.release_id,
                    "failed to reap stale release (likely already transitioned): {e}"
                ),
            }
        }

        let stuck = self
            .release_event_store
            .find_stuck_releases(
                self.assigned_timeout.as_secs() as i64,
                self.running_timeout.as_secs() as i64,
            )
            .await?;

        for release in stuck {
            let msg = format!("release timed out (was {} for too long)", release.status);
            tracing::warn!(
                release_id = %release.release_id,
                status = %release.status,
                "reaping stuck release"
            );

            match self
                .release_event_store
                .emit_event(
                    release.release_id,
                    ReleaseEventType::TimedOut,
                    EventPayload {
                        error_message: Some(msg.clone()),
                        ..Default::default()
                    },
                    None,
                )
                .await
            {
                Ok(_) => self.notify_failed(release.release_id, &msg).await,
                Err(e) => tracing::debug!(
                    release_id = %release.release_id,
                    "failed to reap release (likely already transitioned): {e}"
                ),
            }
        }

        let orphaned = self
            .release_event_store
            .find_orphaned_active_intents(self.intent_finalize_grace.as_secs() as i64)
            .await?;

        for intent_id in orphaned {
            tracing::warn!(
                %intent_id,
                "finalizing an ACTIVE intent whose releases are all terminal"
            );
            self.release_event_store
                .try_finalize_direct_intent(&intent_id)
                .await;
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
