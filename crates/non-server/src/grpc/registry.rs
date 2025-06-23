use std::pin::Pin;

use anyhow::Context;
use futures::{Stream, TryStreamExt};
use non_grpc_interface::{registry_service_server::RegistryService, *};
use uuid::Uuid;

use crate::{
    services::component_registry::{ComponentRegistryState, FileStream, models::ComponentVersion},
    state::State,
};

pub struct RegistryServer {
    pub state: State,
}

#[async_trait::async_trait]
impl RegistryService for RegistryServer {
    async fn get_components(
        &self,
        _request: tonic::Request<GetComponentsRequest>,
    ) -> std::result::Result<tonic::Response<GetComponentsResponse>, tonic::Status> {
        Ok(tonic::Response::new(GetComponentsResponse {}))
    }

    #[tracing::instrument(skip(self), level = "trace")]
    async fn get_component(
        &self,
        request: tonic::Request<GetComponentRequest>,
    ) -> std::result::Result<tonic::Response<GetComponentResponse>, tonic::Status> {
        tracing::info!("get component");
        let request = request.into_inner();

        let component = self
            .state
            .component_registry()
            .get_component(&request.name, &request.namespace)
            .await
            .inspect_err(|e| tracing::warn!("failed to get components: {:#?}", e))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(GetComponentResponse {
            component: component.map(|c| c.into()),
        }))
    }

    async fn begin_upload(
        &self,
        request: tonic::Request<BeginUploadRequest>,
    ) -> std::result::Result<tonic::Response<BeginUploadResponse>, tonic::Status> {
        let request = request.into_inner();

        let context = self
            .state
            .component_registry()
            .begin_upload(&request.name, &request.namespace, &request.version)
            .await
            .inspect_err(|e| tracing::warn!("failed to get components: {:#?}", e))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(BeginUploadResponse {
            upload_context: context.into(),
        }))
    }

    async fn upload_file(
        &self,
        request: tonic::Request<UploadFileRequest>,
    ) -> std::result::Result<tonic::Response<UploadFileResponse>, tonic::Status> {
        let request = request.into_inner();

        self.state
            .component_registry()
            .upload_file(
                request
                    .upload_context
                    .try_into()
                    .map_err(|e: anyhow::Error| tonic::Status::invalid_argument(e.to_string()))?,
                request.file_path,
                &request.file_content,
            )
            .await
            .inspect_err(|e| tracing::warn!("failed to upload file: {:#?}", e))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(UploadFileResponse {}))
    }

    async fn commit_upload(
        &self,
        request: tonic::Request<CommitUploadRequest>,
    ) -> std::result::Result<tonic::Response<CommitUploadResponse>, tonic::Status> {
        let request = request.into_inner();

        self.state
            .component_registry()
            .commit(
                request
                    .upload_context
                    .try_into()
                    .map_err(|e: anyhow::Error| tonic::Status::invalid_argument(e.to_string()))?,
            )
            .await
            .inspect_err(|e| tracing::warn!("failed to commit upload: {:#?}", e))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(CommitUploadResponse {}))
    }

    type GetComponentFilesStream = Pin<
        Box<
            dyn Stream<Item = std::result::Result<GetComponentFilesResponse, tonic::Status>> + Send,
        >,
    >;
    async fn get_component_files(
        &self,
        request: tonic::Request<GetComponentFilesRequest>,
    ) -> std::result::Result<tonic::Response<Self::GetComponentFilesStream>, tonic::Status> {
        let request = request.into_inner();

        let mut stream = FileStream::new();

        let take_stream = stream.take_stream();

        let component_id: Uuid = request
            .component_id
            .parse()
            .context("failed to parse uuid")
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;

        let s = self.state.clone();
        tokio::spawn(async move {
            if let Err(e) = s.component_registry().get_files(component_id, stream).await {
                tracing::error!("failed to send files: {:#?}", e);
            }
        });

        Ok(tonic::Response::new(take_stream))
    }
}

impl From<ComponentVersion> for Component {
    fn from(value: ComponentVersion) -> Self {
        Self {
            id: value.id,
            version: value.version,
        }
    }
}
