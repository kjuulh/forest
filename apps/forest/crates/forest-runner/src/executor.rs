use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use anyhow::Context;
use forest_grpc_interface::{ReleaseMode, ReleaseOutcome};

use crate::backend::DestinationConfig;
use crate::backend::remote::RemoteBackend;
use crate::backend::{ProjectInfo, ReleaseAnnotation};
use crate::client::RunnerSession;
use crate::destinations::{RunnerContext, RunnerDestination};
use crate::logger::RemoteLogger;

pub struct Executor {
    destinations: Vec<Box<dyn RunnerDestination>>,
    active_count: Arc<AtomicI32>,
}

impl Executor {
    pub fn new(destinations: Vec<Box<dyn RunnerDestination>>) -> Self {
        Self {
            destinations,
            active_count: Arc::new(AtomicI32::new(0)),
        }
    }

    pub fn active_count(&self) -> i32 {
        self.active_count.load(Ordering::Relaxed)
    }

    pub async fn execute(
        &self,
        session: &mut RunnerSession,
        assignment: &forest_grpc_interface::WorkAssignment,
    ) -> anyhow::Result<()> {
        let release_token = &assignment.release_token;
        let destination_info = assignment
            .destination
            .as_ref()
            .context("work assignment missing destination info")?;

        let dest_type = destination_info
            .r#type
            .as_ref()
            .context("destination info missing type")?;

        let handler = self
            .destinations
            .iter()
            .find(|d| {
                d.capabilities().iter().any(|cap| {
                    cap.organisation == dest_type.organisation
                        && cap.name == dest_type.name
                        && cap.version == dest_type.version
                })
            })
            .context("no destination handler matches the assignment")?;

        self.active_count.fetch_add(1, Ordering::Relaxed);
        let _guard = ActiveGuard(self.active_count.clone());

        let log_sender = session
            .open_log_stream()
            .await
            .context("failed to open log stream")?;
        let logger = RemoteLogger::new(release_token.clone(), log_sender);

        tracing::info!(
            release_token,
            destination = destination_info.name,
            "fetching release data"
        );

        let deployment_files = session
            .get_release_files(release_token)
            .await
            .context("failed to fetch deployment files")?;

        let spec_files = session
            .get_spec_files(release_token)
            .await
            .context("failed to fetch spec files")?;

        let annotation_resp = session
            .get_release_annotation(release_token)
            .await
            .context("failed to fetch release annotation")?;

        let (org, project) = session
            .get_project_info(release_token)
            .await
            .context("failed to fetch project info")?;

        let annotation = ReleaseAnnotation {
            slug: annotation_resp.slug,
            source_username: non_empty(annotation_resp.source_username),
            source_email: non_empty(annotation_resp.source_email),
            context_title: non_empty(annotation_resp.context_title),
            context_description: non_empty(annotation_resp.context_description),
            context_web: non_empty(annotation_resp.context_web),
            reference_version: non_empty(annotation_resp.reference_version),
            reference_commit_sha: non_empty(annotation_resp.reference_commit_sha),
            reference_commit_branch: non_empty(annotation_resp.reference_commit_branch),
            reference_commit_message: non_empty(annotation_resp.reference_commit_message),
            created_at: annotation_resp.created_at,
        };

        let project_info = ProjectInfo {
            organisation: org,
            project,
        };

        let temp_dir = std::env::temp_dir().join(format!("forest-runner-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir)
            .await
            .context("failed to create temp directory")?;

        let backend = RemoteBackend::new(
            deployment_files,
            spec_files,
            annotation,
            project_info,
            logger.clone(),
            temp_dir.clone(),
        );

        let config = DestinationConfig {
            name: destination_info.name.clone(),
            environment: destination_info.environment.clone(),
            metadata: destination_info.metadata.clone(),
            organisation: dest_type.organisation.clone(),
            type_name: dest_type.name.clone(),
            type_version: dest_type.version,
        };

        let ctx = RunnerContext {
            release_token: release_token.clone(),
            destination: config,
            backend: Box::new(backend),
        };

        let is_plan_mode = ReleaseMode::try_from(assignment.mode).unwrap_or(ReleaseMode::Deploy)
            == ReleaseMode::Plan;

        let result = if is_plan_mode {
            run_destination_plan(handler.as_ref(), &ctx).await
        } else {
            run_destination(handler.as_ref(), &ctx).await.map(|()| None)
        };

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;

        match &result {
            Ok(plan_output) => {
                tracing::info!(release_token, "release completed successfully");
                session
                    .complete_release(
                        release_token,
                        ReleaseOutcome::Success,
                        None,
                        plan_output.as_deref(),
                    )
                    .await?;
            }
            Err(e) => {
                let error_msg = format!("{e:#}");
                tracing::error!(release_token, error = %error_msg, "release failed");
                logger.log_stderr(&format!("ERROR: {error_msg}"));
                session
                    .complete_release(
                        release_token,
                        ReleaseOutcome::Failure,
                        Some(&error_msg),
                        None,
                    )
                    .await?;
            }
        }

        result.map(|_| ())
    }
}

async fn run_destination(
    handler: &dyn RunnerDestination,
    ctx: &RunnerContext,
) -> anyhow::Result<()> {
    handler
        .prepare(ctx)
        .await
        .context("destination prepare failed")?;
    handler
        .release(ctx)
        .await
        .context("destination release failed")?;
    Ok(())
}

async fn run_destination_plan(
    handler: &dyn RunnerDestination,
    ctx: &RunnerContext,
) -> anyhow::Result<Option<String>> {
    handler
        .prepare(ctx)
        .await
        .context("destination prepare failed")?;
    handler.plan(ctx).await.context("destination plan failed")
}

struct ActiveGuard(Arc<AtomicI32>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}
