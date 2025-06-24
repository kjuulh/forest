use std::pin::Pin;

use crate::{
    repositories::{
        components::{ComponentsRepository, ComponentsRepositoryState},
        files::{FilesRepository, FilesRepositoryState},
        staging::{ComponentStagingRepository, ComponentStagingRepositoryState},
    },
    state::State,
};

pub mod models {
    pub struct ComponentVersion {
        pub id: String,
        pub name: String,
        pub namespace: String,
        pub version: String,
    }
}
use anyhow::Context;
use futures::{SinkExt, Stream};
use models::*;
use non_grpc_interface::GetComponentFilesResponse;
use uuid::Uuid;

pub struct ComponentRegistry {
    component_repository: ComponentsRepository,
    staging: ComponentStagingRepository,
    files: FilesRepository,
}

impl ComponentRegistry {
    pub async fn get_component(
        &self,
        component_name: &str,
        component_namespace: &str,
    ) -> anyhow::Result<Option<ComponentVersion>> {
        let component = self
            .component_repository
            .get_component(component_name, component_namespace)
            .await?;

        Ok(component)
    }

    pub async fn get_component_version(
        &self,
        name: &str,
        namespace: &str,
        version: &str,
    ) -> anyhow::Result<Option<ComponentVersion>> {
        let component = self
            .component_repository
            .get_component_version(name, namespace, version)
            .await?;

        Ok(component)
    }

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn begin_upload(
        &self,
        name: &str,
        namespace: &str,
        version: &str,
    ) -> anyhow::Result<UploadContext> {
        tracing::debug!("beginning upload");
        let context = self
            .staging
            .create_staging(name, namespace, version)
            .await?;

        Ok(UploadContext { context })
    }

    #[tracing::instrument(skip(self, file_content), level = "trace")]
    pub async fn upload_file(
        &self,
        context: UploadContext,
        file_path: String,
        file_content: &[u8],
    ) -> anyhow::Result<()> {
        tracing::debug!("uploading file");

        self.files
            .upload(&context.context, &file_path, file_content)
            .await
            .context(format!("failed to upload file: {}", &file_path))?;

        Ok(())
    }

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn commit(&self, context: UploadContext) -> anyhow::Result<()> {
        tracing::debug!("commiting upload");

        self.staging.commit_staging(&context.context).await?;

        Ok(())
    }

    #[tracing::instrument(skip(self, file_stream), level = "trace")]
    pub async fn get_files(
        &self,
        component_id: Uuid,
        file_stream: FileStream,
    ) -> anyhow::Result<()> {
        tracing::debug!("getting files");

        let mut page = 0;
        loop {
            let file = match self.files.get_file(&component_id, page).await {
                Ok(Some(r)) => r,
                Ok(None) => {
                    tracing::debug!("component has no more files, exiting");
                    break;
                }
                Err(e) => {
                    file_stream.push_err(e).await?;
                    break;
                }
            };

            match file_stream.push_file(&file.path, &file.content).await {
                Ok(_) => {}
                Err(e) => {
                    file_stream.push_err(e).await?;
                    break;
                }
            }

            page += 1;
        }

        file_stream.push_done().await?;

        Ok(())
    }
}

pub struct FileStream {
    rx: Option<
        futures::channel::mpsc::Receiver<
            std::result::Result<GetComponentFilesResponse, tonic::Status>,
        >,
    >,
    tx: futures::channel::mpsc::Sender<
        std::result::Result<GetComponentFilesResponse, tonic::Status>,
    >,
}

impl FileStream {
    pub fn new() -> Self {
        let (tx, rx) = futures::channel::mpsc::channel(10);

        Self { tx, rx: Some(rx) }
    }

    pub fn take_stream(
        &mut self,
    ) -> Pin<
        Box<
            dyn Stream<Item = std::result::Result<GetComponentFilesResponse, tonic::Status>> + Send,
        >,
    > {
        Box::pin(self.rx.take().expect("to only take stream once"))
    }

    pub async fn push_err(&self, error: anyhow::Error) -> anyhow::Result<()> {
        self.tx
            .clone()
            .send(Err(tonic::Status::internal(error.to_string())))
            .await?;

        Ok(())
    }

    pub async fn push_file(&self, file_path: &str, file_content: &[u8]) -> anyhow::Result<()> {
        self.tx
            .clone()
            .send(Ok(GetComponentFilesResponse {
                msg: Some(
                    non_grpc_interface::get_component_files_response::Msg::ComponentFile(
                        non_grpc_interface::ComponentFile {
                            file_path: file_path.into(),
                            file_content: file_content.into(),
                        },
                    ),
                ),
            }))
            .await?;

        Ok(())
    }

    pub async fn push_done(mut self) -> anyhow::Result<()> {
        self.tx
            .send(Ok(GetComponentFilesResponse {
                msg: Some(non_grpc_interface::get_component_files_response::Msg::Done(
                    non_grpc_interface::Done {},
                )),
            }))
            .await?;

        self.tx.close_channel();

        Ok(())
    }
}

#[derive(Debug)]
pub struct UploadContext {
    context: Uuid,
}

impl TryFrom<String> for UploadContext {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Ok(Self {
            context: value.parse()?,
        })
    }
}

impl From<UploadContext> for String {
    fn from(value: UploadContext) -> Self {
        value.context.to_string()
    }
}

pub trait ComponentRegistryState {
    fn component_registry(&self) -> ComponentRegistry;
}

impl ComponentRegistryState for State {
    fn component_registry(&self) -> ComponentRegistry {
        ComponentRegistry {
            component_repository: self.components_repository(),
            staging: self.component_staging_repository(),
            files: self.files_repository(),
        }
    }
}
