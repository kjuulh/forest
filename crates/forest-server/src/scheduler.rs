use std::time::Duration;

use anyhow::Context;
use forest_grpc_interface::{DestinationInfo, WorkAssignment};
use forest_models::ReleaseStatus;
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::{
    State,
    destination_services::{DestinationServices, DestinationServicesState},
    destinations::{DestinationIndex, logger::DestinationLogger},
    runner_manager::RunnerManager,
    services::{
        destination_registry::{DestinationRegistry, DestinationRegistryState},
        notification_registry::{NotificationRegistry, NotificationRegistryState},
        release_finalizer,
        release_logs_registry::{ReleaseLogsRegistry, ReleaseLogsRegistryState},
        release_registry::{ReleaseItem, ReleaseRegistry, ReleaseRegistryState},
        release_token_registry::{
            ReleaseTokenRegistry, ReleaseTokenRegistryState, ReleaseTokenScope,
        },
    },
};

enum ScheduleResult {
    /// Release was executed in-process and completed (success).
    InProcessComplete,
    /// Release was assigned to a remote runner. Status is now RUNNING.
    /// The runner will call CompleteRelease when done.
    RemoteAssigned,
}

pub struct Scheduler {
    release_registry: ReleaseRegistry,
    release_log_registry: ReleaseLogsRegistry,
    destination_registry: DestinationRegistry,
    notification_registry: NotificationRegistry,
    destinations: DestinationServices,
    runner_manager: RunnerManager,
    release_token_registry: ReleaseTokenRegistry,
    disable_in_process: bool,
}

impl Scheduler {
    pub fn new(state: &State, runner_manager: RunnerManager, disable_in_process: bool) -> Self {
        use crate::destination_services::DestinationServicesState;
        use crate::services::destination_registry::DestinationRegistryState;
        use crate::services::notification_registry::NotificationRegistryState;
        use crate::services::release_logs_registry::ReleaseLogsRegistryState;
        use crate::services::release_registry::ReleaseRegistryState;
        use crate::services::release_token_registry::ReleaseTokenRegistryState;

        Self {
            release_registry: state.release_registry(),
            release_log_registry: state.release_logs_registry(),
            destinations: state.destination_services(),
            destination_registry: state.destination_registry(),
            notification_registry: state.notification_registry(),
            runner_manager,
            release_token_registry: state.release_token_registry(),
            disable_in_process,
        }
    }

    pub async fn handle(&self, _cancellation: &CancellationToken) -> anyhow::Result<()> {
        let Some((staged_release, tx)) = self.release_registry.get_staged_release().await? else {
            return Ok(());
        };

        tracing::info!(id =% staged_release.id, "begin processing release");

        let res = self.schedule_destination(&staged_release).await;
        match res {
            Ok(ScheduleResult::RemoteAssigned) => {
                // Remote runner took the work. Transition to RUNNING.
                // The runner's CompleteRelease call will finalize status + notifications.
                self.release_registry
                    .commit_release_status(&staged_release, tx, ReleaseStatus::Running)
                    .await?;
            }
            Ok(ScheduleResult::InProcessComplete) => {
                // In-process execution succeeded. Commit SUCCESS + notification.
                self.release_registry
                    .commit_release_status(&staged_release, tx, ReleaseStatus::Success)
                    .await?;

                // Fire-and-forget notification
                if let Err(e) = release_finalizer::send_notification(
                    &self.release_registry,
                    &self.notification_registry,
                    &self.destination_registry,
                    &staged_release,
                    ReleaseStatus::Success,
                    None,
                )
                .await
                {
                    tracing::warn!("failed to create success notification: {e:#}");
                }
            }
            Err(e) => {
                tracing::warn!("failed to handle release: {e:#}");

                self.release_registry
                    .commit_release_status(&staged_release, tx, ReleaseStatus::Failure)
                    .await?;

                if let Err(e2) = release_finalizer::send_notification(
                    &self.release_registry,
                    &self.notification_registry,
                    &self.destination_registry,
                    &staged_release,
                    ReleaseStatus::Failure,
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

    async fn schedule_destination(
        &self,
        staged_release: &ReleaseItem,
    ) -> anyhow::Result<ScheduleResult> {
        let dest = self
            .destination_registry
            .get(&staged_release.destination_id)
            .await?
            .context("failed to find a destination")?;

        let dest_index = DestinationIndex {
            organisation: dest.destination_type.organisation.clone(),
            name: dest.destination_type.name.clone(),
            version: dest.destination_type.version,
        };

        // Try remote runner first
        if let Some((runner_id, work_sender)) = self.runner_manager.try_assign(&dest_index).await {
            // Create a scoped token for this release
            let token = self
                .release_token_registry
                .create_token(
                    ReleaseTokenScope {
                        release_id: staged_release.id,
                        release_intent_id: staged_release.release_intent_id,
                        artifact_id: staged_release.artifact,
                        destination_id: staged_release.destination_id,
                        project_id: staged_release.project_id,
                        environment: dest.environment.clone(),
                        runner_id: runner_id.clone(),
                    },
                    Duration::from_secs(3600), // 1 hour TTL
                )
                .await?;

            let dest_cap = forest_grpc_interface::DestinationCapability {
                organisation: dest.destination_type.organisation.clone(),
                name: dest.destination_type.name.clone(),
                version: dest.destination_type.version as u64,
            };

            let assignment = WorkAssignment {
                release_token: token,
                release_id: staged_release.id.to_string(),
                release_intent_id: staged_release.release_intent_id.to_string(),
                artifact_id: staged_release.artifact.to_string(),
                destination_id: staged_release.destination_id.to_string(),
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
                        release_id = %staged_release.id,
                        destination = %dest.name,
                        "assigned release to remote runner"
                    );
                    return Ok(ScheduleResult::RemoteAssigned);
                }
                Err(_) => {
                    // Runner channel closed — fall through to in-process
                    tracing::warn!(
                        runner_id = %runner_id,
                        "failed to send work to runner (channel closed), falling back to in-process"
                    );
                }
            }
        }

        // Fallback: in-process execution
        if self.disable_in_process {
            anyhow::bail!(
                "no remote runner available for {} and in-process execution is disabled",
                dest_index
            );
        }

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
            DestinationLogger::new(staged_release.clone(), self.release_log_registry.clone());

        dest_svc.prepare(&logger, staged_release, &dest).await?;
        dest_svc.release(&logger, staged_release, &dest).await?;

        tracing::info!("release to destination success (in-process)");

        Ok(ScheduleResult::InProcessComplete)
    }
}

impl Component for Scheduler {
    fn info(&self) -> ComponentInfo {
        "forest-server/scheduler".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    break;
                }
                _ = interval.tick() => {
                    self.handle(&cancellation_token).await?;
                }
            }
        }

        Ok(())
    }
}

pub trait SchedulerState {
    fn scheduler(&self, runner_manager: RunnerManager, disable_in_process: bool) -> Scheduler;
}

impl SchedulerState for State {
    fn scheduler(&self, runner_manager: RunnerManager, disable_in_process: bool) -> Scheduler {
        Scheduler {
            release_registry: self.release_registry(),
            release_log_registry: self.release_logs_registry(),
            destinations: self.destination_services(),
            destination_registry: self.destination_registry(),
            notification_registry: self.notification_registry(),
            runner_manager,
            release_token_registry: self.release_token_registry(),
            disable_in_process,
        }
    }
}
