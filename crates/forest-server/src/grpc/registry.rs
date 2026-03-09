use std::pin::Pin;

use anyhow::Context;
use futures::Stream;
use forest_grpc_interface::{registry_service_server::RegistryService, *};
use uuid::Uuid;

use crate::{
    services::component_aggregate::{
        ComponentServiceState, ComponentVersion, FileStream,
    },
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
            .component_service()
            .get_component(&request.name, &request.organisation)
            .await
            .inspect_err(|e| tracing::warn!("failed to get component: {e:#}"))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(GetComponentResponse {
            component: component.map(|c| c.into()),
        }))
    }

    async fn get_component_version(
        &self,
        request: tonic::Request<GetComponentVersionRequest>,
    ) -> std::result::Result<tonic::Response<GetComponentVersionResponse>, tonic::Status> {
        let req = request.into_inner();

        let component = self
            .state
            .component_service()
            .get_component_version(&req.name, &req.organisation, &req.version)
            .await
            .inspect_err(|e| tracing::warn!("failed to get component version: {e:#}"))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(GetComponentVersionResponse {
            component: component.map(|c| c.into()),
        }))
    }

    async fn begin_upload(
        &self,
        request: tonic::Request<BeginUploadRequest>,
    ) -> std::result::Result<tonic::Response<BeginUploadResponse>, tonic::Status> {
        let request = request.into_inner();

        let upload_id = self
            .state
            .component_service()
            .begin_upload(&request.organisation, &request.name, &request.version)
            .await
            .inspect_err(|e| tracing::warn!("failed to begin upload: {e:#}"))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(BeginUploadResponse {
            upload_context: upload_id.to_string(),
        }))
    }

    async fn upload_file(
        &self,
        request: tonic::Request<UploadFileRequest>,
    ) -> std::result::Result<tonic::Response<UploadFileResponse>, tonic::Status> {
        let request = request.into_inner();

        let upload_id: Uuid = request
            .upload_context
            .parse()
            .context("invalid upload_context UUID")
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;

        self.state
            .component_service()
            .upload_file(upload_id, &request.file_path, &request.file_content)
            .await
            .inspect_err(|e| tracing::warn!("failed to upload file: {e:#}"))
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(UploadFileResponse {}))
    }

    async fn commit_upload(
        &self,
        request: tonic::Request<CommitUploadRequest>,
    ) -> std::result::Result<tonic::Response<CommitUploadResponse>, tonic::Status> {
        let request = request.into_inner();

        let upload_id: Uuid = request
            .upload_context
            .parse()
            .context("invalid upload_context UUID")
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;

        self.state
            .component_service()
            .commit_upload(upload_id)
            .await
            .inspect_err(|e| tracing::warn!("failed to commit upload: {e:#}"))
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

        let service = self.state.component_service();
        tokio::spawn(async move {
            if let Err(e) = service.get_files(component_id, stream).await {
                tracing::error!("failed to send files: {e:#}");
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
