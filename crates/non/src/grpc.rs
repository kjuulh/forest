use std::{collections::HashMap, path::Path, sync::OnceLock};

use anyhow::Context;
use futures::{SinkExt, Stream, TryStreamExt};
use non_grpc_interface::{
    AnnotateReleaseRequest, BeginUploadArtifactRequest, BeginUploadRequest, CommitArtifactRequest,
    CommitUploadRequest, Component, ComponentFile, CreateDestinationRequest, CreateRequest,
    GetArtifactBySlugRequest, GetComponentFilesRequest, GetComponentRequest,
    GetComponentVersionRequest, GetDestinationsRequest, GetNamespacesRequest, GetProjectsRequest,
    ReleaseRequest, UpdateDestinationRequest, UploadArtifactRequest, UploadArtifactResponse,
    UploadFileRequest, artifact_service_client::ArtifactServiceClient,
    destination_service_client::DestinationServiceClient, get_component_files_response::Msg,
    get_projects_request::Query, namespace_service_client::NamespaceServiceClient,
    registry_service_client::RegistryServiceClient, release_service_client::ReleaseServiceClient,
};
use non_models::{Destination, DestinationType, Namespace, ProjectName};
use tokio::{
    sync::{OnceCell, mpsc::Sender},
    task::JoinHandle,
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Response, transport::Channel};
use uuid::Uuid;

use crate::{
    models::{
        self, artifacts::ArtifactID, context::ArtifactContext,
        release_annotation::ReleaseAnnotation, source::Source,
    },
    state::State,
};

#[derive(Clone)]
pub struct GrpcClient {
    host: String,
    namespaces_client: OnceCell<NamespaceServiceClient<Channel>>,
    registry_client: OnceCell<RegistryServiceClient<Channel>>,
    artifact_client: OnceCell<ArtifactServiceClient<Channel>>,
    release_client: OnceCell<ReleaseServiceClient<Channel>>,
    destination_client: OnceCell<DestinationServiceClient<Channel>>,
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

    pub async fn get_component_version(
        &self,
        name: &str,
        namespace: &str,
        version: &str,
    ) -> anyhow::Result<Option<Component>> {
        let mut client = self.registry_client().await?;

        let resp = client
            .get_component_version(GetComponentVersionRequest {
                name: name.into(),
                namespace: namespace.into(),
                version: version.into(),
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

    pub async fn begin_artifact_upload(&self) -> anyhow::Result<UploadFileHandle> {
        let mut client = self.artifact_client().await?;

        let resp = client
            .begin_upload_artifact(BeginUploadArtifactRequest {})
            .await?;

        let resp = resp.into_inner();

        let (tx, rx) = tokio::sync::mpsc::channel::<UploadArtifactRequest>(10);

        let handle = tokio::spawn(async move {
            let stream_req = ReceiverStream::new(rx);
            client
                .upload_artifact(stream_req)
                .await
                .context("upload artifact")
                .inspect_err(|e| tracing::error!("failed to upload file: {:?}", e))
        });

        Ok(UploadFileHandle {
            tx,
            handle,
            staging_id: resp.upload_id,
        })
    }

    #[tracing::instrument(skip(self, handle, file_content), level = "trace")]
    pub async fn upload_artifact_file(
        &self,
        handle: &UploadFileHandle,
        file_name: &str,
        file_content: &str,
        env: &str,
        destination: &str,
    ) -> anyhow::Result<()> {
        tracing::info!("uploading file: {}", handle.staging_id);

        handle
            .tx
            .send(UploadArtifactRequest {
                upload_id: handle.staging_id.clone(),
                env: env.into(),
                destination: destination.into(),
                file_name: file_name.into(),
                file_content: file_content.into(),
            })
            .await?;

        Ok(())
    }

    pub async fn commit_artifact_upload(
        &self,
        handle: UploadFileHandle,
    ) -> anyhow::Result<ArtifactID> {
        let staging_id = handle.staging_id;
        let upload_file_handle = handle.handle;

        drop(handle.tx);

        // Make sure we've received a final response from the server
        // FIXME: this may block forever if there are multiple producers
        upload_file_handle.await??;

        tracing::info!("commiting upload: {}", staging_id);

        let mut client = self.artifact_client().await?;
        let res = client
            .commit_artifact(CommitArtifactRequest {
                upload_id: staging_id,
            })
            .await
            .context("grpc commit artifact")?;

        let msg = res.into_inner();

        Ok(msg.artifact_id.try_into()?)
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

    async fn release_client(&self) -> anyhow::Result<ReleaseServiceClient<Channel>> {
        let client = self
            .release_client
            .get_or_try_init(move || async move {
                let channel = Channel::from_shared(self.host.clone())?.connect().await?;
                let client = ReleaseServiceClient::new(channel);

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
    async fn artifact_client(&self) -> anyhow::Result<ArtifactServiceClient<Channel>> {
        let client = self
            .artifact_client
            .get_or_try_init(move || async move {
                let channel = Channel::from_shared(self.host.clone())?.connect().await?;
                let client = ArtifactServiceClient::new(channel);

                Ok::<_, anyhow::Error>(client)
            })
            .await?;

        Ok(client.clone())
    }
    async fn destination_client(&self) -> anyhow::Result<DestinationServiceClient<Channel>> {
        let client = self
            .destination_client
            .get_or_try_init(move || async move {
                let channel = Channel::from_shared(self.host.clone())?.connect().await?;
                let client = DestinationServiceClient::new(channel);

                Ok::<_, anyhow::Error>(client)
            })
            .await?;

        Ok(client.clone())
    }

    pub async fn list_files(
        &self,
        component_id: &str,
        f: impl Fn(ComponentFile),
    ) -> anyhow::Result<()> {
        let mut client = self.registry_client().await?;
        let resp = client
            .get_component_files(GetComponentFilesRequest {
                component_id: component_id.into(),
            })
            .await?;

        let mut stream = resp.into_inner();
        while let Some(msg) = stream.message().await? {
            let Some(msg) = msg.msg else { return Ok(()) };

            match msg {
                Msg::Done(_) => {
                    tracing::info!("done receiving items");
                    break;
                }
                Msg::ComponentFile(component_file) => {
                    f(component_file);
                }
            }
        }

        Ok(())
    }

    pub async fn get_component_files(
        &self,
        component_id: &str,
    ) -> anyhow::Result<impl Stream<Item = anyhow::Result<ComponentFile>>> {
        let mut client = self.registry_client().await?;
        let resp = client
            .get_component_files(GetComponentFilesRequest {
                component_id: component_id.into(),
            })
            .await?;

        let (mut tx, rx) = futures::channel::mpsc::channel(10);

        tokio::spawn(async move {
            let mut stream = resp.into_inner();
            loop {
                let Ok(message) = stream.message().await else {
                    tx.send(Err(anyhow::anyhow!("failed to read next item")))
                        .await
                        .expect("failed to send end result");
                    tx.close_channel();

                    return;
                };
                if let Some(msg) = message {
                    let Some(msg) = msg.msg else {
                        return;
                    };

                    match msg {
                        Msg::Done(_) => {
                            tracing::info!("done receiving items");
                            tx.close_channel();

                            break;
                        }
                        Msg::ComponentFile(component_file) => {
                            tx.send(Ok(component_file))
                                .await
                                .expect("to be able to send item");
                        }
                    }
                } else {
                    break;
                }
            }

            tx.close_channel();
        });

        Ok(rx.into_stream())
    }

    pub async fn annotate_artifact(
        &self,
        artifact_id: &ArtifactID,
        metadata: &HashMap<String, String>,
        source: &Source,
        context: &ArtifactContext,
        project: &models::project::Project,
        reference: &models::reference::Reference,
    ) -> anyhow::Result<String> {
        let mut client = self.release_client().await?;

        let resp = client
            .annotate_release(AnnotateReleaseRequest {
                artifact_id: artifact_id.to_string(),
                metadata: metadata.clone(),
                source: Some(source.clone().into()),
                context: Some(context.clone().into()),
                project: Some(project.clone().into()),
                r#ref: Some(reference.clone().into()),
            })
            .await
            .context("annotate artifact")?;

        let resp = resp.into_inner();

        Ok(resp.artifact.context("no artifact found")?.slug)
    }

    pub async fn get_release_annotation_by_slug(
        &self,
        slug: &str,
    ) -> anyhow::Result<ReleaseAnnotation> {
        let mut client = self.release_client().await?;

        let resp = client
            .get_artifact_by_slug(GetArtifactBySlugRequest { slug: slug.into() })
            .await
            .context("get release annotation by slug")?;

        let res = resp.into_inner();

        res.artifact
            .ok_or(anyhow::anyhow!("artifact could not be found"))?
            .try_into()
            .context("release annotation")
    }

    pub async fn release(
        &self,
        artifact_id: Uuid,
        destination: &[String],
        environments: &[String],
    ) -> anyhow::Result<()> {
        let mut client = self.release_client().await?;

        client
            .release(ReleaseRequest {
                artifact_id: artifact_id.to_string(),
                destinations: destination.into(),
                environments: environments.into(),
            })
            .await
            .context("release (grpc)")?;

        Ok(())
    }

    pub async fn get_namespaces(&self) -> anyhow::Result<Vec<Namespace>> {
        let mut client = self.release_client().await?;

        let response = client
            .get_namespaces(GetNamespacesRequest {})
            .await
            .context("get namespaces (grpc)")?;
        let resp = response.into_inner();

        Ok(resp.namespaces.into_iter().map(|r| r.into()).collect())
    }

    pub async fn get_projects(&self, query: GetProjectsQuery) -> anyhow::Result<Vec<ProjectName>> {
        let mut client = self.release_client().await?;

        let response = client
            .get_projects(GetProjectsRequest {
                query: Some(match query {
                    GetProjectsQuery::Namespace(ns) => Query::Namespace(ns.into()),
                }),
            })
            .await
            .context("get projects (grpc)")?;
        let resp = response.into_inner();

        Ok(resp.projects.into_iter().map(|r| r.into()).collect())
    }

    pub async fn get_destinations(&self) -> anyhow::Result<Vec<Destination>> {
        let mut client = self.destination_client().await?;

        let response = client
            .get_destinations(GetDestinationsRequest {})
            .await
            .context("get destinations (grpc)")?;
        let resp = response.into_inner();

        Ok(resp
            .destinations
            .into_iter()
            .map(|r| {
                Destination::new(
                    &r.name,
                    &r.environment,
                    r.metadata,
                    r.r#type.expect("to always be available").into(),
                )
            })
            .collect())
    }

    pub async fn create_destination(
        &self,
        name: &str,
        environment: &str,
        metadata: HashMap<String, String>,
        destination_type: DestinationType,
    ) -> anyhow::Result<()> {
        self.destination_client()
            .await?
            .create_destination(CreateDestinationRequest {
                name: name.to_string(),
                environment: environment.to_string(),
                metadata,
                r#type: Some(destination_type.into()),
            })
            .await
            .context("create destination (grpc)")?;

        Ok(())
    }

    pub async fn update_destination(
        &self,
        name: &str,
        metadata: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        self.destination_client()
            .await?
            .update_destination(UpdateDestinationRequest {
                name: name.to_string(),
                metadata,
            })
            .await
            .context("create destination (grpc)")?;

        Ok(())
    }
}

pub enum GetProjectsQuery {
    Namespace(Namespace),
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
                artifact_client: OnceCell::const_new(),
                release_client: OnceCell::const_new(),
                destination_client: OnceCell::const_new(),
            }
        })
        .clone()
    }
}

pub struct UploadFileHandle {
    tx: Sender<UploadArtifactRequest>,
    handle: JoinHandle<Result<Response<UploadArtifactResponse>, anyhow::Error>>,
    staging_id: String,
}

impl From<crate::models::context::ArtifactContext> for non_grpc_interface::ArtifactContext {
    fn from(value: crate::models::context::ArtifactContext) -> Self {
        Self {
            title: value.title,
            description: value.description,
            web: value.web,
        }
    }
}

impl From<crate::models::source::Source> for non_grpc_interface::Source {
    fn from(value: crate::models::source::Source) -> Self {
        Self {
            user: value.username,
            email: value.email,
        }
    }
}

impl TryFrom<non_grpc_interface::Artifact> for models::release_annotation::ReleaseAnnotation {
    type Error = anyhow::Error;

    fn try_from(value: non_grpc_interface::Artifact) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.parse().context("id")?,
            artifact_id: value.artifact_id.parse().context("artifact id")?,
            slug: value.slug,
            metadata: value.metadata,
            source: value.source.context("source not found")?.into(),
            context: value.context.context("context not found")?.into(),
        })
    }
}

impl From<non_grpc_interface::Source> for models::source::Source {
    fn from(value: non_grpc_interface::Source) -> Self {
        Self {
            username: value.user,
            email: value.email,
        }
    }
}

impl From<non_grpc_interface::ArtifactContext> for models::context::ArtifactContext {
    fn from(value: non_grpc_interface::ArtifactContext) -> Self {
        Self {
            title: value.title,
            description: value.description,
            web: value.web,
        }
    }
}

impl From<crate::models::project::Project> for non_grpc_interface::Project {
    fn from(value: crate::models::project::Project) -> Self {
        Self {
            namespace: value.namespace,
            project: value.project,
        }
    }
}
impl From<non_grpc_interface::Project> for crate::models::project::Project {
    fn from(value: non_grpc_interface::Project) -> Self {
        Self {
            namespace: value.namespace,
            project: value.project,
        }
    }
}

impl From<non_grpc_interface::Ref> for crate::models::reference::Reference {
    fn from(value: non_grpc_interface::Ref) -> Self {
        Self {
            commit_sha: value.commit_sha,
            commit_branch: value.branch,
        }
    }
}
impl From<crate::models::reference::Reference> for non_grpc_interface::Ref {
    fn from(value: crate::models::reference::Reference) -> Self {
        Self {
            commit_sha: value.commit_sha,
            branch: value.commit_branch,
        }
    }
}
