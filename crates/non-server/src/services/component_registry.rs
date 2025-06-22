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
        pub name: String,
        pub namespace: String,
        pub version: String,
    }
}
use anyhow::Context;
use models::*;
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
