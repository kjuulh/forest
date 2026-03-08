use anyhow::Context;
use futures::StreamExt;
use forest_grpc_interface::{artifact_service_server::ArtifactService, *};
use tonic::Response;

use crate::{
    actor::Actor,
    services::artifact_staging_registry::{ArtifactStagingRegistryState, StagingArtifactID},
    state::State,
};

pub struct ArtifactServer {
    pub state: State,
}

#[async_trait::async_trait]
impl ArtifactService for ArtifactServer {
    async fn begin_upload_artifact(
        &self,
        request: tonic::Request<BeginUploadArtifactRequest>,
    ) -> std::result::Result<tonic::Response<BeginUploadArtifactResponse>, tonic::Status> {
        let actor = request
            .extensions()
            .get::<Actor>()
            .cloned()
            .ok_or_else(|| tonic::Status::unauthenticated("missing actor"))?;

        let _req = request.into_inner();

        let id = self
            .state
            .artifact_staging_registry()
            .create_staging_entry(&actor)
            .await
            .context("create staging entry")
            .to_internal_error()?;

        Ok(Response::new(BeginUploadArtifactResponse {
            upload_id: id.to_string(),
        }))
    }

    async fn upload_artifact(
        &self,
        request: tonic::Request<tonic::Streaming<UploadArtifactRequest>>,
    ) -> std::result::Result<tonic::Response<UploadArtifactResponse>, tonic::Status> {
        let mut req = request.into_inner();

        let staging = self.state.artifact_staging_registry();

        while let Some(msg) = req
            .next()
            .await
            .transpose()
            .inspect_err(|e| tracing::warn!("had error: {}", e))?
        {
            tracing::info!("uploading file: file_name: {} (category: {})", msg.file_name, msg.category);

            let upload_staging_id: StagingArtifactID = msg
                .upload_id
                .try_into()
                .context("artifact id")
                .to_internal_error()?;

            let category = if msg.category.is_empty() {
                "deployment"
            } else {
                &msg.category
            };

            staging
                .upload_file(
                    &upload_staging_id,
                    &msg.file_name,
                    &msg.file_content,
                    &msg.env,
                    &msg.destination,
                    category,
                )
                .await
                .to_internal_error()?;
        }

        Ok(Response::new(UploadArtifactResponse {}))
    }

    async fn commit_artifact(
        &self,
        request: tonic::Request<CommitArtifactRequest>,
    ) -> std::result::Result<tonic::Response<CommitArtifactResponse>, tonic::Status> {
        let req = request.into_inner();

        let upload_staging_id: StagingArtifactID = req
            .upload_id
            .try_into()
            .context("upload id")
            .to_internal_error()?;

        let id = self
            .state
            .artifact_staging_registry()
            .commit_staging(&upload_staging_id)
            .await
            .context("commit staging")
            .to_internal_error()?;

        Ok(Response::new(CommitArtifactResponse {
            artifact_id: id.to_string(),
        }))
    }

    async fn get_artifact_spec(
        &self,
        request: tonic::Request<GetArtifactSpecRequest>,
    ) -> std::result::Result<tonic::Response<GetArtifactSpecResponse>, tonic::Status> {
        let req = request.into_inner();

        let artifact_id: uuid::Uuid = req
            .artifact_id
            .parse()
            .context("artifact_id")
            .to_internal_error()?;

        let spec_files = self
            .state
            .artifact_staging_registry()
            .get_spec_files(&artifact_id)
            .await
            .to_internal_error()?;

        // Return the forest.cue file, or the first spec file if forest.cue isn't found
        let content = spec_files
            .iter()
            .find(|(path, _)| path.file_name().is_some_and(|n| n == "forest.cue"))
            .or_else(|| spec_files.first())
            .map(|(_, content)| content.clone())
            .unwrap_or_default();

        Ok(Response::new(GetArtifactSpecResponse { content }))
    }

    async fn get_artifact_files(
        &self,
        request: tonic::Request<GetArtifactFilesRequest>,
    ) -> std::result::Result<tonic::Response<GetArtifactFilesResponse>, tonic::Status> {
        let req = request.into_inner();

        let artifact_id: uuid::Uuid = req
            .artifact_id
            .parse()
            .context("artifact_id")
            .to_internal_error()?;

        let category = req.category.as_deref().filter(|c| !c.is_empty());

        let files = self
            .state
            .artifact_staging_registry()
            .get_artifact_files(&artifact_id, category)
            .await
            .to_internal_error()?;

        Ok(Response::new(GetArtifactFilesResponse {
            files: files
                .into_iter()
                .map(|f| ArtifactFile {
                    file_name: f.file_name,
                    category: f.category,
                    env: f.env,
                    destination: f.destination,
                    content: f.content,
                })
                .collect(),
        }))
    }
}

pub trait GrpcErrorExt<T> {
    #[allow(clippy::result_large_err)]
    fn to_internal_error(self) -> Result<T, tonic::Status>;
}

impl<T> GrpcErrorExt<T> for anyhow::Result<T> {
    fn to_internal_error(self) -> Result<T, tonic::Status> {
        self.inspect_err(|e| tracing::error!("create staging failed: {:?}", e))
            .map_err(|e| tonic::Status::internal(e.to_string()))
    }
}
