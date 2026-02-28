use anyhow::Context;
use forest_models::ReleaseStatus;
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::{
    State,
    destination_services::{DestinationServices, DestinationServicesState},
    destinations::logger::DestinationLogger,
    services::{
        destination_registry::{DestinationRegistry, DestinationRegistryState},
        notification_registry::{
            NotificationRegistry, NotificationRegistryState,
            ReleaseContext as NotifReleaseContext,
        },
        release_logs_registry::{ReleaseLogsRegistry, ReleaseLogsRegistryState},
        release_registry::{ReleaseItem, ReleaseRegistry, ReleaseRegistryState},
    },
};

pub struct Scheduler {
    release_registry: ReleaseRegistry,
    release_log_registry: ReleaseLogsRegistry,
    destination_registry: DestinationRegistry,
    notification_registry: NotificationRegistry,
    destinations: DestinationServices,
}

impl Scheduler {
    pub async fn handle(&self, _cancellation: &CancellationToken) -> anyhow::Result<()> {
        let Some((staged_release, tx)) = self.release_registry.get_staged_release().await? else {
            return Ok(());
        };

        tracing::info!(id =% staged_release.id, "begin processing release");

        // Get project context, destination name, and annotation context for notifications
        let project_context = self
            .release_registry
            .get_project_context(&staged_release.project_id)
            .await
            .ok();

        let dest = self
            .destination_registry
            .get(&staged_release.destination_id)
            .await
            .ok()
            .flatten();
        let dest_name = dest.as_ref().map(|d| d.name.clone());
        let dest_env = dest.as_ref().map(|d| d.environment.clone());

        let ann_ctx = self
            .release_registry
            .get_annotation_context(&staged_release.artifact)
            .await
            .ok();

        let res = self.schedule_destination(&staged_release).await;
        match res {
            Ok(_) => {
                self.release_registry
                    .commit_release_status(&staged_release, tx, ReleaseStatus::Success)
                    .await?;

                if let Some((ref org, ref project)) = project_context
                    && let Err(e) = self
                        .notification_registry
                        .create_notification(
                            "RELEASE_SUCCEEDED",
                            &format!("Release succeeded: {}/{}", org, project),
                            &format!(
                                "Release {} completed successfully{}",
                                staged_release.id,
                                dest_name
                                    .as_ref()
                                    .map(|d| format!(" to {}", d))
                                    .unwrap_or_default()
                            ),
                            org,
                            project,
                            &NotifReleaseContext {
                                slug: ann_ctx.as_ref().map(|a| a.slug.clone()),
                                artifact_id: Some(staged_release.artifact.to_string()),
                                release_intent_id: Some(
                                    staged_release.release_intent_id.to_string(),
                                ),
                                destination: dest_name.clone(),
                                environment: dest_env.clone(),
                                source_username: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.source.username.clone()),
                                source_email: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.source.email.clone()),
                                source_type: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.source.source_type.clone()),
                                run_url: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.source.run_url.clone()),
                                commit_sha: ann_ctx
                                    .as_ref()
                                    .map(|a| a.reference.commit_sha.clone()),
                                commit_branch: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.reference.commit_branch.clone()),
                                commit_message: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.reference.commit_message.clone()),
                                version: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.reference.version.clone()),
                                repo_url: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.reference.repo_url.clone()),
                                context_title: ann_ctx
                                    .as_ref()
                                    .map(|a| a.context.title.clone()),
                                context_description: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.context.description.clone()),
                                context_web: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.context.web.clone()),
                                context_pr: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.context.pr.clone()),
                                ..Default::default()
                            },
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

                if let Some((ref org, ref project)) = project_context
                    && let Err(e2) = self
                        .notification_registry
                        .create_notification(
                            "RELEASE_FAILED",
                            &format!("Release failed: {}/{}", org, project),
                            &format!(
                                "Release {} failed: {}{}",
                                staged_release.id,
                                e,
                                dest_name
                                    .as_ref()
                                    .map(|d| format!(" (dest: {})", d))
                                    .unwrap_or_default()
                            ),
                            org,
                            project,
                            &NotifReleaseContext {
                                slug: ann_ctx.as_ref().map(|a| a.slug.clone()),
                                artifact_id: Some(staged_release.artifact.to_string()),
                                release_intent_id: Some(
                                    staged_release.release_intent_id.to_string(),
                                ),
                                destination: dest_name.clone(),
                                environment: dest_env.clone(),
                                source_username: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.source.username.clone()),
                                source_email: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.source.email.clone()),
                                source_type: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.source.source_type.clone()),
                                run_url: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.source.run_url.clone()),
                                commit_sha: ann_ctx
                                    .as_ref()
                                    .map(|a| a.reference.commit_sha.clone()),
                                commit_branch: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.reference.commit_branch.clone()),
                                commit_message: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.reference.commit_message.clone()),
                                version: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.reference.version.clone()),
                                repo_url: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.reference.repo_url.clone()),
                                context_title: ann_ctx
                                    .as_ref()
                                    .map(|a| a.context.title.clone()),
                                context_description: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.context.description.clone()),
                                context_web: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.context.web.clone()),
                                context_pr: ann_ctx
                                    .as_ref()
                                    .and_then(|a| a.context.pr.clone()),
                                error_message: Some(format!("{e:#}")),
                                ..Default::default()
                            },
                        )
                        .await
                {
                    tracing::warn!("failed to create failure notification: {e2:#}");
                }
            }
        }

        Ok(())
    }

    async fn schedule_destination(&self, staged_release: &ReleaseItem) -> anyhow::Result<()> {
        let dest = self
            .destination_registry
            .get(&staged_release.destination_id)
            .await?
            .context("failed to find a destination")?;

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

        tracing::info!("release to destination success");

        Ok(())
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
    fn scheduler(&self) -> Scheduler;
}

impl SchedulerState for State {
    fn scheduler(&self) -> Scheduler {
        Scheduler {
            release_registry: self.release_registry(),
            release_log_registry: self.release_logs_registry(),
            destinations: self.destination_services(),
            destination_registry: self.destination_registry(),
            notification_registry: self.notification_registry(),
        }
    }
}
