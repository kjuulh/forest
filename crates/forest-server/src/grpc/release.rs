use anyhow::Context;
use forest_grpc_interface::{release_service_server::ReleaseService, *};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Response;

use crate::{
    grpc::artifacts::GrpcErrorExt,
    services::{
        notification_registry::{NotificationRegistryState, ReleaseContext as NotifReleaseContext},
        release_logs_registry::{LogChannel, ReleaseLogsRegistryState},
        release_registry::{self, ReleaseAnnotation, ReleaseDestination, ReleaseRegistryState},
    },
    state::State,
};

pub struct ReleaseServer {
    pub state: State,
}

#[async_trait::async_trait]
impl ReleaseService for ReleaseServer {
    async fn annotate_release(
        &self,
        request: tonic::Request<AnnotateReleaseRequest>,
    ) -> std::result::Result<tonic::Response<AnnotateReleaseResponse>, tonic::Status> {
        tracing::debug!("annotate release");

        let req = request.into_inner();

        let slug = petname::petname(3, "-").expect("to be able to generate slug");

        let proj = req
            .project
            .context("no project found")
            .to_internal_error()?;

        let artifact_id = req
            .artifact_id
            .parse::<uuid::Uuid>()
            .context("artifact id")
            .to_internal_error()?;

        // Extract source/context/ref info for both the annotate call and notification context
        let source: release_registry::Source = req
            .source
            .map(|s| s.into())
            .context("source is required")
            .to_internal_error()?;
        let art_context: release_registry::ArtifactContext = req
            .context
            .map(|s| s.into())
            .context("context is required")
            .to_internal_error()?;
        let reference: release_registry::Reference = req
            .r#ref
            .map(|r| r.into())
            .context("ref is required")
            .to_internal_error()?;

        let artifact = self
            .state
            .release_registry()
            .annotate(
                &artifact_id,
                &slug,
                &req.metadata,
                &source,
                &art_context,
                &proj.organisation,
                &proj.project,
                &reference,
            )
            .await
            .to_internal_error()?;

        if let Err(e) = self
            .state
            .notification_registry()
            .create_notification(
                "RELEASE_ANNOTATED",
                &format!("Artifact annotated: {}", slug),
                &format!(
                    "Artifact {} annotated in {}/{}",
                    slug, &proj.organisation, &proj.project
                ),
                &proj.organisation,
                &proj.project,
                &NotifReleaseContext {
                    slug: Some(slug.clone()),
                    artifact_id: Some(artifact_id.to_string()),
                    source_username: source.username.clone(),
                    source_email: source.email.clone(),
                    commit_sha: Some(reference.commit_sha.clone()),
                    commit_branch: reference.commit_branch.clone(),
                    context_title: Some(art_context.title.clone()),
                    context_description: art_context.description.clone(),
                    context_web: art_context.web.clone(),
                    ..Default::default()
                },
            )
            .await
        {
            tracing::warn!("failed to create annotation notification: {e:#}");
        }

        Ok(Response::new(AnnotateReleaseResponse {
            artifact: Some(artifact.into()),
        }))
    }

    async fn get_artifact_by_slug(
        &self,
        request: tonic::Request<GetArtifactBySlugRequest>,
    ) -> std::result::Result<tonic::Response<GetArtifactBySlugResponse>, tonic::Status> {
        tracing::debug!("get artifact by slug");
        let req = request.into_inner();

        let release_annotation = self
            .state
            .release_registry()
            .get_release_annotation_by_slug(&req.slug)
            .await
            .context("get release annotation by slug")
            .to_internal_error()?;

        Ok(Response::new(GetArtifactBySlugResponse {
            artifact: Some(release_annotation.into()),
        }))
    }
    async fn get_artifacts_by_project(
        &self,
        request: tonic::Request<GetArtifactsByProjectRequest>,
    ) -> std::result::Result<tonic::Response<GetArtifactsByProjectResponse>, tonic::Status> {
        tracing::debug!("get artifact by project");
        let req = request.into_inner();

        let project = req
            .project
            .ok_or(anyhow::anyhow!("project is required"))
            .to_internal_error()?;

        let release_annotation = self
            .state
            .release_registry()
            .get_release_annotation_by_project(&project.organisation, &project.project)
            .await
            .context("get release annotation by project")
            .to_internal_error()?;

        Ok(Response::new(GetArtifactsByProjectResponse {
            artifact: release_annotation.into_iter().map(|r| r.into()).collect(),
        }))
    }

    async fn release(
        &self,
        request: tonic::Request<ReleaseRequest>,
    ) -> std::result::Result<tonic::Response<ReleaseResponse>, tonic::Status> {
        tracing::debug!("release");
        let req = request.into_inner();

        let artifact_id: uuid::Uuid = req
            .artifact_id
            .parse()
            .context("artifact id")
            .to_internal_error()?;

        let created = self
            .state
            .release_registry()
            .release(&artifact_id, req.destinations, req.environments)
            .await
            .context("release")
            .to_internal_error()?;

        let dest_count = created.releases.len();
        let dest_names: Vec<String> = created
            .releases
            .iter()
            .map(|r| r.destination.clone())
            .collect();

        // Fetch annotation context to enrich the started notification
        let ann_ctx = self
            .state
            .release_registry()
            .get_annotation_context(&artifact_id)
            .await
            .ok();

        if let Err(e) = self
            .state
            .notification_registry()
            .create_notification(
                "RELEASE_STARTED",
                &format!(
                    "Release started: {}/{}",
                    &created.organisation, &created.project
                ),
                &format!("Release staged to {} destination(s)", dest_count),
                &created.organisation,
                &created.project,
                &NotifReleaseContext {
                    slug: ann_ctx.as_ref().map(|a| a.slug.clone()),
                    artifact_id: Some(artifact_id.to_string()),
                    release_intent_id: Some(created.release_intent_id.to_string()),
                    destination: if dest_names.len() == 1 {
                        Some(dest_names[0].clone())
                    } else {
                        None
                    },
                    destination_count: dest_count as i32,
                    source_username: ann_ctx.as_ref().and_then(|a| a.source.username.clone()),
                    source_email: ann_ctx.as_ref().and_then(|a| a.source.email.clone()),
                    commit_sha: ann_ctx.as_ref().map(|a| a.reference.commit_sha.clone()),
                    commit_branch: ann_ctx
                        .as_ref()
                        .and_then(|a| a.reference.commit_branch.clone()),
                    context_title: ann_ctx.as_ref().map(|a| a.context.title.clone()),
                    context_description: ann_ctx
                        .as_ref()
                        .and_then(|a| a.context.description.clone()),
                    context_web: ann_ctx.as_ref().and_then(|a| a.context.web.clone()),
                    ..Default::default()
                },
            )
            .await
        {
            tracing::warn!("failed to create release started notification: {e:#}");
        }

        Ok(Response::new(ReleaseResponse {
            intents: created
                .releases
                .into_iter()
                .map(|r| ReleaseIntent {
                    release_intent_id: created.release_intent_id.to_string(),
                    destination: r.destination,
                    environment: r.environment,
                })
                .collect(),
        }))
    }

    type WaitReleaseStream = ReceiverStream<Result<WaitReleaseEvent, tonic::Status>>;

    async fn wait_release(
        &self,
        request: tonic::Request<WaitReleaseRequest>,
    ) -> std::result::Result<tonic::Response<Self::WaitReleaseStream>, tonic::Status> {
        tracing::debug!("wait_release stream");
        let req = request.into_inner();

        let release_intent_id: uuid::Uuid = req
            .release_intent_id
            .parse()
            .context("release_intent_id")
            .to_internal_error()?;

        let (tx, rx) = mpsc::channel(32);
        let release_registry = self.state.release_registry();
        let logs_registry = self.state.release_logs_registry();

        // Spawn a task to poll and send status updates and logs for all destinations
        tokio::spawn(async move {
            let poll_interval = std::time::Duration::from_millis(500);
            // Track last status per destination
            let mut last_statuses: std::collections::HashMap<
                uuid::Uuid,
                forest_models::ReleaseStatus,
            > = std::collections::HashMap::new();
            // Track log cursors per destination
            let mut log_cursors: std::collections::HashMap<uuid::Uuid, i64> =
                std::collections::HashMap::new();

            loop {
                match release_registry
                    .get_release_status_by_intent(&release_intent_id)
                    .await
                {
                    Ok(status_infos) => {
                        if status_infos.is_empty() {
                            // No releases found yet for this intent, keep polling
                            tokio::time::sleep(poll_interval).await;
                            continue;
                        }

                        let mut all_finalized = true;

                        for status_info in &status_infos {
                            // Check if status changed for this destination
                            let status_changed = last_statuses
                                .get(&status_info.destination_id)
                                .map(|prev| *prev != status_info.status)
                                .unwrap_or(true);

                            if status_changed {
                                last_statuses
                                    .insert(status_info.destination_id, status_info.status);

                                let event = WaitReleaseEvent {
                                    event: Some(wait_release_event::Event::StatusUpdate(
                                        ReleaseStatusUpdate {
                                            destination: status_info.destination.clone(),
                                            status: status_info.status.to_string(),
                                        },
                                    )),
                                };

                                if tx.send(Ok(event)).await.is_err() {
                                    return;
                                }
                            }

                            // Poll for new log blocks for this destination
                            let log_cursor =
                                *log_cursors.get(&status_info.destination_id).unwrap_or(&-1);

                            match logs_registry
                                .get_logs_after_sequence(
                                    release_intent_id,
                                    status_info.destination_id,
                                    log_cursor,
                                )
                                .await
                            {
                                Ok(log_blocks) => {
                                    for block in log_blocks {
                                        // Update cursor for this destination
                                        if block.sequence > log_cursor {
                                            log_cursors
                                                .insert(status_info.destination_id, block.sequence);
                                        }

                                        // Send each log line from the block
                                        for log_line in block.log_lines {
                                            let event = WaitReleaseEvent {
                                                event: Some(wait_release_event::Event::LogLine(
                                                    ReleaseLogLine {
                                                        destination: status_info.destination.clone(),
                                                        line: log_line.line,
                                                        timestamp: log_line.timestamp.to_string(),
                                                        channel: match log_line.channel {
                                                            LogChannel::Stdout => {
                                                                forest_grpc_interface::LogChannel::Stdout
                                                                    .into()
                                                            }
                                                            LogChannel::Stderr => {
                                                                forest_grpc_interface::LogChannel::Stderr
                                                                    .into()
                                                            }
                                                        },
                                                    },
                                                )),
                                            };

                                            if tx.send(Ok(event)).await.is_err() {
                                                return;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        destination = %status_info.destination,
                                        "error polling logs: {e:#}"
                                    );
                                }
                            }

                            if !status_info.status.is_finalized() {
                                all_finalized = false;
                            }
                        }

                        // If all destinations are finalized, we're done
                        if all_finalized {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("error polling release status: {e:#}");
                        let _ = tx
                            .send(Err(tonic::Status::internal(format!(
                                "error polling status: {e}"
                            ))))
                            .await;
                        break;
                    }
                }

                tokio::time::sleep(poll_interval).await;
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn get_organisations(
        &self,
        request: tonic::Request<GetOrganisationsRequest>,
    ) -> std::result::Result<tonic::Response<GetOrganisationsResponse>, tonic::Status> {
        tracing::debug!("get organisations");
        let _req = request.into_inner();

        let organisations = self
            .state
            .release_registry()
            .get_organisations()
            .await
            .context("failed to find organisations")
            .to_internal_error()?;

        Ok(Response::new(GetOrganisationsResponse {
            organisations: organisations.into_iter().map(|n| n.into()).collect(),
        }))
    }

    async fn get_projects(
        &self,
        request: tonic::Request<GetProjectsRequest>,
    ) -> std::result::Result<tonic::Response<GetProjectsResponse>, tonic::Status> {
        let req = request.into_inner();
        tracing::debug!("get projects: {req:?}");

        let projects = match req.query.context("query is required").to_internal_error()? {
            get_projects_request::Query::Organisation(organisation) => self
                .state
                .release_registry()
                .get_projects_by_organisation(&organisation.into())
                .await
                .context("failed to find projects")
                .to_internal_error()?,
        };

        Ok(Response::new(GetProjectsResponse {
            projects: projects.into_iter().map(|n| n.to_string()).collect(),
        }))
    }
}

impl From<grpc::ArtifactContext> for crate::services::release_registry::ArtifactContext {
    fn from(value: grpc::ArtifactContext) -> Self {
        Self {
            title: value.title,
            description: value.description,
            web: value.web,
        }
    }
}

impl From<grpc::Source> for crate::services::release_registry::Source {
    fn from(value: grpc::Source) -> Self {
        Self {
            username: value.user,
            email: value.email,
        }
    }
}

impl From<grpc::Ref> for crate::services::release_registry::Reference {
    fn from(value: grpc::Ref) -> Self {
        Self {
            commit_sha: value.commit_sha,
            commit_branch: value.branch,
        }
    }
}

impl From<ReleaseAnnotation> for Artifact {
    fn from(value: ReleaseAnnotation) -> Self {
        Self {
            id: value.id.into(),
            artifact_id: value.artifact_id.into(),
            metadata: value.metadata,
            source: Some(value.source.into()),
            context: Some(value.context.into()),
            slug: value.slug,
            project: Some(value.project.into()),
            destinations: value.destinations.into_iter().map(|d| d.into()).collect(),
            created_at: value.created_at.to_rfc3339(),
        }
    }
}

impl From<ReleaseDestination> for ArtifactDestination {
    fn from(value: ReleaseDestination) -> Self {
        Self {
            name: value.name,
            environment: value.environment,
            type_organisation: value.type_organisation,
            type_name: value.type_name,
            type_version: value.type_version as u64,
        }
    }
}

impl From<release_registry::Source> for grpc::Source {
    fn from(value: release_registry::Source) -> Self {
        Self {
            user: value.username,
            email: value.email,
        }
    }
}

impl From<release_registry::ArtifactContext> for grpc::ArtifactContext {
    fn from(value: release_registry::ArtifactContext) -> Self {
        Self {
            title: value.title,
            description: value.description,
            web: value.web,
        }
    }
}

impl From<release_registry::Project> for grpc::Project {
    fn from(value: release_registry::Project) -> Self {
        Self {
            organisation: value.organisation,
            project: value.project,
        }
    }
}
