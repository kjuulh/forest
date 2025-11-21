use anyhow::Context;
use non_grpc_interface::{release_service_server::ReleaseService, *};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Response;

use crate::{
    grpc::artifacts::GrpcErrorExt,
    services::release_registry::{
        self, ReleaseAnnotation, ReleaseDestination, ReleaseRegistryState,
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

        let artifact = self
            .state
            .release_registry()
            .annotate(
                &req.artifact_id
                    .parse::<uuid::Uuid>()
                    .context("artifact id")
                    .to_internal_error()?,
                &slug,
                &req.metadata,
                &req.source
                    .map(|s| s.into())
                    .context("source is required")
                    .to_internal_error()?,
                &req.context
                    .map(|s| s.into())
                    .context("context is required")
                    .to_internal_error()?,
                &proj.namespace,
                &proj.project,
                &req.r#ref
                    .map(|r| r.into())
                    .context("ref is required")
                    .to_internal_error()?,
            )
            .await
            .to_internal_error()?;

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
            .get_release_annotation_by_project(&project.namespace, &project.project)
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

        self.state
            .release_registry()
            .release(
                &req.artifact_id
                    .parse()
                    .context("artifact id")
                    .to_internal_error()?,
                req.destinations,
                req.environments,
            )
            .await
            .context("release")
            .to_internal_error()?;

        Ok(Response::new(ReleaseResponse {}))
    }

    type WaitReleaseStream = ReceiverStream<Result<WaitReleaseEvent, tonic::Status>>;

    async fn wait_release(
        &self,
        request: tonic::Request<WaitReleaseRequest>,
    ) -> std::result::Result<tonic::Response<Self::WaitReleaseStream>, tonic::Status> {
        tracing::debug!("wait_release stream");
        let req = request.into_inner();

        let artifact_id: uuid::Uuid = req
            .artifact_id
            .parse()
            .context("artifact id")
            .to_internal_error()?;
        let environment = req.environment;

        let (tx, rx) = mpsc::channel(32);
        let release_registry = self.state.release_registry();

        // Spawn a task to poll and send status updates
        tokio::spawn(async move {
            let poll_interval = std::time::Duration::from_millis(1000);
            let mut last_status: Option<non_models::ReleaseStatus> = None;

            loop {
                match release_registry
                    .get_release_status(&artifact_id, &environment)
                    .await
                {
                    Ok(Some(status_info)) => {
                        // Only send if status changed
                        let status_changed = last_status
                            .map(|prev| prev != status_info.status)
                            .unwrap_or(true);

                        if status_changed {
                            last_status = Some(status_info.status);

                            let event = WaitReleaseEvent {
                                event: Some(wait_release_event::Event::StatusUpdate(
                                    ReleaseStatusUpdate {
                                        destination: status_info.destination.clone(),
                                        status: status_info.status.to_string(),
                                    },
                                )),
                            };

                            if tx.send(Ok(event)).await.is_err() {
                                // Client disconnected
                                break;
                            }

                            // If finalized, we're done
                            if status_info.status.is_finalized() {
                                break;
                            }
                        }
                    }
                    Ok(None) => {
                        // No release found yet, keep polling
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

    async fn get_namespaces(
        &self,
        request: tonic::Request<GetNamespacesRequest>,
    ) -> std::result::Result<tonic::Response<GetNamespacesResponse>, tonic::Status> {
        tracing::debug!("get namespaces");
        let _req = request.into_inner();

        let namespaces = self
            .state
            .release_registry()
            .get_namespaces()
            .await
            .context("failed to find namespaces")
            .to_internal_error()?;

        Ok(Response::new(GetNamespacesResponse {
            namespaces: namespaces.into_iter().map(|n| n.into()).collect(),
        }))
    }

    async fn get_projects(
        &self,
        request: tonic::Request<GetProjectsRequest>,
    ) -> std::result::Result<tonic::Response<GetProjectsResponse>, tonic::Status> {
        tracing::debug!("get projects");
        let req = request.into_inner();

        let projects = match req.query.context("query is required").to_internal_error()? {
            get_projects_request::Query::Namespace(namespace) => self
                .state
                .release_registry()
                .get_projects_by_namespace(&namespace.into())
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
            namespace: value.namespace,
            project: value.project,
        }
    }
}
