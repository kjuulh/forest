use std::{sync::Arc, time::Duration};

use anyhow::Context;
use forest_grpc_interface::{
    DestinationInfo, ReleaseArtifactStore, ReleaseMode, WorkAssignment,
};
use forest_models::ReleaseStatus;
use futures::StreamExt;
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    State,
    destination_services::{DestinationServices, DestinationServicesState},
    destinations::{
        DestinationIndex,
        logger::DestinationLogger,
        terraformv1::{TerraformStateStore, TerraformStateStoreState},
    },
    runner_manager::RunnerManager,
    services::{
        destination_registry::{DestinationRegistry, DestinationRegistryState},
        notification_registry::{NotificationRegistry, NotificationRegistryState},
        release_event_store::{
            EventPayload, ReleaseEventStore, ReleaseEventStoreState, ReleaseEventType,
        },
        release_finalizer,
        release_logs_registry::{ReleaseLogsRegistry, ReleaseLogsRegistryState},
        policy::{PolicyRegistry, PolicyRegistryState, PolicyType},
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
    policy_registry: PolicyRegistry,
    /// Owned by terraformv1's in-process backend; the scheduler reads it to
    /// hand state-backend credentials to remote runners (e.g. hollow) via
    /// WorkAssignment.terraform_state. Cheap clone (Arcs inside).
    tf_state: TerraformStateStore,
    nats: async_nats::Client,
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
                policy_registry: state.policy_registry(),
                tf_state: state.terraform_state_store(),
                nats: state.nats.clone(),
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

        // Check soak_time policies before dispatching.
        // Branch restriction is enforced at the gRPC layer where branch info is available;
        // the scheduler only handles soak_time deferral.
        let evaluations = self
            .policy_registry
            .evaluate_for_environment(
                &release_state.project_id,
                &dest.environment,
                None,
                None,
            )
            .await
            .unwrap_or_default();

        for eval in &evaluations {
            if !eval.passed && eval.policy_type == PolicyType::SoakTime {
                tracing::debug!(
                    %release_id,
                    policy = %eval.policy_name,
                    env = %dest.environment,
                    "scheduler: release deferred by soak_time policy — {}",
                    eval.reason,
                );
                return Ok(());
            }
        }

        let dest_index = DestinationIndex {
            organisation: dest.destination_type.organisation.clone(),
            name: dest.destination_type.name.clone(),
            version: dest.destination_type.version,
        };

        let (_, project_name) = self
            .release_registry
            .get_project_context(&release_state.project_id)
            .await
            .unwrap_or_else(|_| (dest.organisation.clone(), "unknown".into()));

        let release_item = ReleaseItem {
            id: release_id,
            release_intent_id: release_state.release_intent_id,
            artifact: release_state.artifact_id,
            project_id: release_state.project_id,
            destination_id: release_state.destination_id,
            status: release_state.status.clone(),
            project: project_name,
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

            let mode = if release_state.mode == "plan" {
                ReleaseMode::Plan
            } else {
                ReleaseMode::Deploy
            };

            // Populate the artifact-store handle for destination types that
            // need server-managed state. Today only `forest/terraform/<v>`;
            // we add arms here as new destination types come online. The
            // shape (URL + basic auth) is generic on purpose — the runner
            // translates to destination-specific env vars.
            let artifact_store = if dest.destination_type.organisation == "forest"
                && dest.destination_type.name == "terraform"
            {
                let project_id = release_state.project_id.to_string();
                let state_id = TerraformStateStore::state_id_for(&dest.environment, &project_id);
                let (id, password) = self.tf_state.urls(state_id).await;
                let url = format!(
                    "{}/{id}",
                    self.tf_state.external_url.trim_end_matches('/')
                );
                Some(ReleaseArtifactStore {
                    id,
                    url,
                    username: "forest-terraform-v1".to_string(),
                    password,
                })
            } else {
                None
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
                mode: mode.into(),
                artifact_store,
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

        let is_plan_mode = release_state.mode == "plan";

        let result = async {
            if is_plan_mode {
                // Plan mode: run the plan phase only, capture output
                dest_svc.prepare(&logger, &release_item, &dest).await?;
                let plan_output = dest_svc.plan(&logger, &release_item, &dest).await?;
                if let Some(output) = plan_output {
                    sqlx::query!(
                        "UPDATE release_states SET plan_output = $2 WHERE release_id = $1",
                        release_id,
                        output,
                    )
                    .execute(&self.release_event_store.db)
                    .await?;
                }
            } else {
                // Normal deploy mode
                dest_svc.prepare(&logger, &release_item, &dest).await?;
                dest_svc.release(&logger, &release_item, &dest).await?;

                // Seed a PENDING health observation so the CLI can start watching immediately
                if let Err(e) = crate::services::release_health::seed_pending(
                    &self.release_event_store.db,
                    &self.nats,
                    release_item.release_intent_id,
                    release_item.id,
                    &dest.name,
                    &dest.environment,
                    &dest.organisation,
                    &release_item.project,
                )
                .await
                {
                    tracing::warn!(%release_id, error = %e, "failed to seed health observation");
                }
            }
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
