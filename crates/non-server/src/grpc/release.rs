use anyhow::Context;
use non_grpc_interface::{release_service_server::ReleaseService, *};
use tonic::Response;

use crate::{
    grpc::artifacts::GrpcErrorExt,
    services::release_registry::{self, ReleaseAnnotation, ReleaseRegistryState},
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
        let req = request.into_inner();

        let slug = petname::petname(3, "-").expect("to be able to generate slug");

        let proj = req
            .project
            .context("no project found")
            .to_internal_error()?;

        self.state
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

        Ok(Response::new(AnnotateReleaseResponse {}))
    }

    async fn get_artifact_by_slug(
        &self,
        request: tonic::Request<GetArtifactBySlugRequest>,
    ) -> std::result::Result<tonic::Response<GetArtifactBySlugResponse>, tonic::Status> {
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

    async fn release(
        &self,
        request: tonic::Request<ReleaseRequest>,
    ) -> std::result::Result<tonic::Response<ReleaseResponse>, tonic::Status> {
        let req = request.into_inner();

        self.state
            .release_registry()
            .release(
                &req.artifact_id
                    .parse()
                    .context("artifact id")
                    .to_internal_error()?,
                req.destinations,
            )
            .await
            .context("release")
            .to_internal_error()?;

        Ok(Response::new(ReleaseResponse {}))
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
