use anyhow::Context;
use futures::StreamExt;
use non_grpc_interface::{artifact_service_server::ArtifactService, *};
use tonic::Response;

use crate::{
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
        let _req = request.into_inner();

        let id = self
            .state
            .artifact_staging_registry()
            .create_staging_entry()
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
            tracing::info!("uploading file: file_name: {}", msg.file_name);

            let upload_staging_id: StagingArtifactID = msg
                .upload_id
                .try_into()
                .context("artifact id")
                .to_internal_error()?;

            staging
                .upload_file(
                    &upload_staging_id,
                    &msg.file_name,
                    &msg.file_content,
                    &msg.env,
                    &msg.destination,
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
