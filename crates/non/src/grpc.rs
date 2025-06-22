use std::{path::Path, sync::OnceLock};

use non_grpc_interface::{
    BeginUploadRequest, CommitUploadRequest, Component, CreateRequest, GetComponentRequest,
    UploadFileRequest, namespace_service_client::NamespaceServiceClient,
    registry_service_client::RegistryServiceClient,
};
use tokio::sync::OnceCell;
use tonic::transport::Channel;

use crate::state::State;

#[derive(Clone)]
pub struct GrpcClient {
    host: String,
    namespaces_client: OnceCell<NamespaceServiceClient<Channel>>,
    registry_client: OnceCell<RegistryServiceClient<Channel>>,
}

impl GrpcClient {
    pub async fn create_namespace(&self, namespace: &str) -> anyhow::Result<()> {
        let mut namespaces_client = self.namespaces_client().await?;

        namespaces_client
            .create(CreateRequest {
                namespace: namespace.into(),
            })
            .await?;

        Ok(())
    }

    pub async fn get_component(
        &self,
        name: &str,
        namespace: &str,
    ) -> anyhow::Result<Option<Component>> {
        let mut client = self.registry_client().await?;

        let resp = client
            .get_component(GetComponentRequest {
                name: name.into(),
                namespace: namespace.into(),
            })
            .await?;

        let resp = resp.into_inner();

        Ok(resp.component)
    }

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn begin_upload(
        &self,
        name: &str,
        namespace: &str,
        version: &str,
    ) -> anyhow::Result<UploadContext> {
        let mut client = self.registry_client().await?;

        tracing::debug!("beginning upload");

        let res = client
            .begin_upload(BeginUploadRequest {
                name: name.into(),
                namespace: namespace.into(),
                version: version.into(),
            })
            .await?;

        Ok(UploadContext {
            context_id: res.into_inner().upload_context.parse()?,
        })
    }

    #[tracing::instrument(skip(self, file_path, file_content), level = "trace")]
    pub async fn upload_file(
        &self,
        context: &UploadContext,
        file_path: &Path,
        file_content: &[u8],
    ) -> anyhow::Result<()> {
        let mut client = self.registry_client().await?;

        tracing::debug!("uploading file");

        client
            .upload_file(UploadFileRequest {
                upload_context: context.into(),
                file_path: file_path.to_string_lossy().to_string(),
                file_content: file_content.into(),
            })
            .await?;

        Ok(())
    }

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn commit_upload(&self, context: &UploadContext) -> anyhow::Result<()> {
        let mut client = self.registry_client().await?;

        tracing::debug!("commit upload");

        client
            .commit_upload(CommitUploadRequest {
                upload_context: context.into(),
            })
            .await?;

        Ok(())
    }

    async fn namespaces_client(&self) -> anyhow::Result<NamespaceServiceClient<Channel>> {
        let client = self
            .namespaces_client
            .get_or_try_init(move || async move {
                let channel = Channel::from_shared(self.host.clone())?.connect().await?;
                let client = NamespaceServiceClient::new(channel);

                Ok::<_, anyhow::Error>(client)
            })
            .await?;

        Ok(client.clone())
    }
    async fn registry_client(&self) -> anyhow::Result<RegistryServiceClient<Channel>> {
        let client = self
            .registry_client
            .get_or_try_init(move || async move {
                let channel = Channel::from_shared(self.host.clone())?.connect().await?;
                let client = RegistryServiceClient::new(channel);

                Ok::<_, anyhow::Error>(client)
            })
            .await?;

        Ok(client.clone())
    }
}

#[derive(Clone, Debug)]
pub struct UploadContext {
    context_id: uuid::Uuid,
}

impl From<UploadContext> for String {
    fn from(value: UploadContext) -> Self {
        value.context_id.to_string()
    }
}

impl From<&UploadContext> for String {
    fn from(value: &UploadContext) -> Self {
        value.context_id.to_string()
    }
}

pub trait GrpcClientState {
    fn grpc_client(&self) -> GrpcClient;
}

impl GrpcClientState for State {
    fn grpc_client(&self) -> GrpcClient {
        static GRPC: OnceLock<GrpcClient> = OnceLock::new();

        GRPC.get_or_init(move || {
            tracing::trace!("creating grpc client");

            GrpcClient {
                // TODO: get from global config
                host: "http://localhost:4040".into(),

                namespaces_client: OnceCell::const_new(),
                registry_client: OnceCell::const_new(),
            }
        })
        .clone()
    }
}
