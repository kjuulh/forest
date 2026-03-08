use std::{sync::Arc, time::Duration};

use anyhow::Context;
use forest_grpc_interface::{DestinationInfo, WorkAssignment};
use forest_models::ReleaseStatus;
use futures::StreamExt;
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    State,
    destination_services::{DestinationServices, DestinationServicesState},
    destinations::{DestinationIndex, logger::DestinationLogger},
    runner_manager::RunnerManager,
    services::{
        destination_registry::{DestinationRegistry, DestinationRegistryState},
        notification_registry::{NotificationRegistry, NotificationRegistryState},
        release_event_store::{
            EventPayload, ReleaseEventStore, ReleaseEventStoreState, ReleaseEventType,
        },
        release_finalizer,
        release_logs_registry::{ReleaseLogsRegistry, ReleaseLogsRegistryState},
        release_registry::{ReleaseItem, ReleaseRegistry, ReleaseRegistryState},
        release_token_registry::{
            ReleaseTokenRegistry, ReleaseTokenRegistryState, ReleaseTokenScope,
        },
    },
};

#[derive(Clone)]
struct SchedulerInner {
    release_registry: ReleaseRegistry,
    release_log_registry: ReleaseLogsRegistry,
    destination_registry: DestinationRegistry,
    notification_registry: NotificationRegistry,
    destinations: DestinationServices,
    runner_manager: RunnerManager,
    release_token_registry: ReleaseTokenRegistry,
    release_event_store: ReleaseEventStore,
    disable_in_process: bool,
}

pub struct Scheduler {
    inner: Arc<SchedulerInner>,
    nats: async_nats::Client,
}

impl Scheduler {
    pub fn new(state: &State, runner_manager: RunnerManager, disable_in_process: bool) -> Self {
        Self {
            inner: Arc::new(SchedulerInner {
                release_registry: state.release_registry(),
                release_log_registry: state.release_logs_registry(),
                destinations: state.destination_services(),
                destination_registry: state.destination_registry(),
                notification_registry: state.notification_registry(),
                runner_manager,
                release_token_registry: state.release_token_registry(),
                release_event_store: state.release_event_store(),
                disable_in_process,
            }),
            nats: state.nats.clone(),
        }
    }

    fn spawn_handle_release(&self, release_id: Uuid) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            if let Err(e) = inner.handle_release(release_id).await {
                tracing::warn!(%release_id, "failed to handle release: {e:#}");
            }
        });
    }
}

impl SchedulerInner {
    /// Handle a specific release by ID.
    async fn handle_release(&self, release_id: Uuid) -> anyhow::Result<()> {
        tracing::debug!(%release_id, "scheduler picked up release");

        let release_state = match self.release_event_store.get_release_state(&release_id).await {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(%release_id, "release not found or already processed: {e:#}");
                return Ok(());
            }
        };

        if release_state.status != "QUEUED" {
            tracing::debug!(%release_id, status = %release_state.status, "skipping release (not QUEUED)");
            return Ok(());
        }

        tracing::info!(%release_id, project_id = %release_state.project_id, destination_id = %release_state.destination_id, "processing queued release");

        let dest = self
            .destination_registry
            .get(&release_state.destination_id)
            .await?
            .context("failed to find a destination")?;

        let dest_index = DestinationIndex {
            organisation: dest.destination_type.organisation.clone(),
            name: dest.destination_type.name.clone(),
            version: dest.destination_type.version,
        };

        let release_item = ReleaseItem {
            id: release_id,
            release_intent_id: release_state.release_intent_id,
            artifact: release_state.artifact_id,
            project_id: release_state.project_id,
            destination_id: release_state.destination_id,
            status: release_state.status.clone(),
        };

        // Try remote runner first
        if let Some((runner_id, work_sender)) = self.runner_manager.try_assign(&dest_index).await {
            // Transition QUEUED -> ASSIGNED
            if let Err(e) = self
                .release_event_store
                .emit_event(
                    release_id,
                    ReleaseEventType::Assigned,
                    EventPayload {
                        runner_id: Some(runner_id.clone()),
                        ..Default::default()
                    },
                    None,
                )
                .await
            {
                // Another instance already transitioned this release
                tracing::debug!(%release_id, "skipping release (already transitioned): {e}");
                return Ok(());
            }

            // Create a scoped token for this release
            let token = self
                .release_token_registry
                .create_token(
                    ReleaseTokenScope {
                        release_id,
                        release_intent_id: release_state.release_intent_id,
                        artifact_id: release_state.artifact_id,
                        destination_id: release_state.destination_id,
                        project_id: release_state.project_id,
                        environment: dest.environment.clone(),
                        runner_id: runner_id.clone(),
                    },
                    Duration::from_secs(3600),
                )
                .await?;

            let dest_cap = forest_grpc_interface::DestinationCapability {
                organisation: dest.destination_type.organisation.clone(),
                name: dest.destination_type.name.clone(),
                version: dest.destination_type.version as u64,
            };

            let assignment = WorkAssignment {
                release_token: token,
                release_id: release_id.to_string(),
                release_intent_id: release_state.release_intent_id.to_string(),
                artifact_id: release_state.artifact_id.to_string(),
                destination_id: release_state.destination_id.to_string(),
                destination: Some(DestinationInfo {
                    name: dest.name.clone(),
                    environment: dest.environment.clone(),
                    metadata: dest.metadata.clone(),
                    r#type: Some(dest_cap),
                    organisation: dest.organisation.clone(),
                }),
            };

            match work_sender.send(assignment).await {
                Ok(()) => {
                    tracing::info!(
                        runner_id = %runner_id,
                        %release_id,
                        destination = %dest.name,
                        "assigned release to remote runner"
                    );
                    return Ok(());
                }
                Err(_) => {
                    // Runner channel closed — fail immediately
                    tracing::warn!(
                        runner_id = %runner_id,
                        "failed to send work to runner (channel closed)"
                    );
                    self.release_event_store
                        .emit_event(
                            release_id,
                            ReleaseEventType::Failed,
                            EventPayload {
                                error_message: Some("runner unavailable (channel closed)".into()),
                                ..Default::default()
                            },
                            None,
                        )
                        .await?;

                    if let Err(e) = release_finalizer::send_notification(
                        &self.release_registry,
                        &self.notification_registry,
                        &self.destination_registry,
                        &release_item,
                        ReleaseStatus::Failed,
                        Some("runner unavailable (channel closed)"),
                    )
                    .await
                    {
                        tracing::warn!("failed to create failure notification: {e:#}");
                    }
                    return Ok(());
                }
            }
        }

        // Fallback: in-process execution
        if self.disable_in_process {
            tracing::info!(
                %release_id,
                destination = %dest_index,
                "no remote runner available, in-process disabled — leaving queued"
            );
            return Ok(());
        }

        tracing::info!(%release_id, destination = %dest.name, "assigning release to in-process executor");

        // Transition QUEUED -> ASSIGNED (in-process)
        if let Err(e) = self
            .release_event_store
            .emit_event(
                release_id,
                ReleaseEventType::Assigned,
                EventPayload {
                    runner_id: Some("in-process".into()),
                    ..Default::default()
                },
                None,
            )
            .await
        {
            tracing::debug!(%release_id, "skipping release (already transitioned): {e}");
            return Ok(());
        }

        // Transition ASSIGNED -> RUNNING
        tracing::debug!(%release_id, "transitioning to RUNNING (in-process)");
        self.release_event_store
            .emit_event(release_id, ReleaseEventType::Started, EventPayload::default(), None)
            .await?;

        let dest_svc = self
            .destinations
            .get_destination(
                &dest.destination_type.organisation,
                &dest.destination_type.name,
                dest.destination_type.version,
            )
            .context(anyhow::anyhow!(
                "no implementation of: {} exists",
                dest.destination_type
            ))?;

        let logger =
            DestinationLogger::new(release_item.clone(), self.release_log_registry.clone());

        // Spawn a heartbeat task that runs alongside the release execution
        let heartbeat_token = tokio_util::sync::CancellationToken::new();
        let heartbeat_cancel = heartbeat_token.clone();
        let heartbeat_store = self.release_event_store.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = heartbeat_cancel.cancelled() => break,
                    _ = interval.tick() => {
                        if let Err(e) = heartbeat_store.heartbeat_release(&release_id).await {
                            tracing::warn!(%release_id, "failed to send heartbeat: {e}");
                        }
                    }
                }
            }
        });

        let result = async {
            dest_svc.prepare(&logger, &release_item, &dest).await?;
            dest_svc.release(&logger, &release_item, &dest).await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;

        heartbeat_token.cancel();

        match result {
            Ok(()) => {
                self.release_event_store
                    .emit_event(
                        release_id,
                        ReleaseEventType::Succeeded,
                        EventPayload::default(),
                        None,
                    )
                    .await?;

                tracing::info!(%release_id, destination = %dest.name, "release succeeded (in-process)");

                if let Err(e) = release_finalizer::send_notification(
                    &self.release_registry,
                    &self.notification_registry,
                    &self.destination_registry,
                    &release_item,
                    ReleaseStatus::Succeeded,
                    None,
                )
                .await
                {
                    tracing::warn!("failed to create success notification: {e:#}");
                }
            }
            Err(e) => {
                tracing::warn!(%release_id, destination = %dest.name, "release failed (in-process): {e:#}");

                self.release_event_store
                    .emit_event(
                        release_id,
                        ReleaseEventType::Failed,
                        EventPayload {
                            error_message: Some(format!("{e:#}")),
                            ..Default::default()
                        },
                        None,
                    )
                    .await?;

                if let Err(e2) = release_finalizer::send_notification(
                    &self.release_registry,
                    &self.notification_registry,
                    &self.destination_registry,
                    &release_item,
                    ReleaseStatus::Failed,
                    Some(&format!("{e:#}")),
                )
                .await
                {
                    tracing::warn!("failed to create failure notification: {e2:#}");
                }
            }
        }

        Ok(())
    }
}

impl Component for Scheduler {
    fn info(&self) -> ComponentInfo {
        "forest-server/scheduler".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        tracing::info!("scheduler starting, subscribing to forest.release.queued");

        let mut subscriber = self
            .nats
            .subscribe("forest.release.queued")
            .await
            .map_err(|e| MadError::Inner(e.into()))?;

        let mut sweep_interval = tokio::time::interval(Duration::from_secs(5));
        sweep_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        tracing::info!(
            in_process = !self.inner.disable_in_process,
            "scheduler ready, sweep interval=5s"
        );

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    break;
                }
                msg = subscriber.next() => {
                    if let Some(msg) = msg {
                        let payload = String::from_utf8_lossy(&msg.payload);
                        if let Ok(release_id) = payload.parse::<Uuid>() {
                            tracing::info!(%release_id, "received release via NATS");
                            self.spawn_handle_release(release_id);
                        } else {
                            tracing::warn!(payload = %payload, "received invalid NATS message (not a UUID)");
                        }
                    }
                }
                _ = sweep_interval.tick() => {
                    let inner = self.inner.clone();
                    tokio::spawn(async move {
                        if let Err(e) = sweep_queued(&inner).await {
                            tracing::error!("scheduler sweep error: {e:#}");
                        }
                    });
                }
            }
        }

        Ok(())
    }
}

/// Fallback sweep: pick up any QUEUED releases that NATS might have missed.
/// Each found release is spawned as an independent task.
async fn sweep_queued(inner: &SchedulerInner) -> anyhow::Result<()> {
    let queued = inner.release_event_store.pick_queued_releases(10).await?;

    if !queued.is_empty() {
        tracing::info!(count = queued.len(), "sweep found queued releases");
    }

    for release in queued {
        let inner = inner.clone();
        let release_id = release.release_id;
        tokio::spawn(async move {
            if let Err(e) = inner.handle_release(release_id).await {
                tracing::warn!(
                    %release_id,
                    "sweep: failed to handle release: {e:#}"
                );
            }
        });
    }

    Ok(())
}

pub trait SchedulerState {
    fn scheduler(&self, runner_manager: RunnerManager, disable_in_process: bool) -> Scheduler;
}

impl SchedulerState for State {
    fn scheduler(&self, runner_manager: RunnerManager, disable_in_process: bool) -> Scheduler {
        Scheduler::new(self, runner_manager, disable_in_process)
    }
}
