use std::{fmt::Display, sync::Arc};

use non_models::Destination;

use crate::{
    State,
    destinations::{kubernetesv1::KubernetesV1Destination, terraformv1::TerraformV1Destination},
    services::{
        artifact_staging_registry::ArtifactStagingRegistryState, release_registry::ReleaseItem,
    },
    temp_dir::TempDirectoriesState,
};

pub mod kubernetesv1;
pub mod terraformv1;

pub struct DestinationService {
    inner: Arc<dyn DestinationEdge + Send + Sync + 'static>,
}

impl DestinationService {
    pub fn new<T: DestinationEdge + Send + Sync + 'static>(t: T) -> Self {
        Self { inner: Arc::new(t) }
    }

    pub fn new_kubernetes_v1() -> Self {
        Self::new(KubernetesV1Destination {})
    }

    pub fn new_terraform_v1(state: &State) -> Self {
        Self::new(TerraformV1Destination {
            temp: state.temp_directories(),
            artifact_files: state.artifact_staging_registry(),
        })
    }

    #[inline(always)]
    pub fn name(&self) -> DestinationIndex {
        self.inner.name()
    }

    pub(crate) async fn prepare(
        &self,
        staged_release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        tracing::debug!(id =% staged_release.id, destination =% self.name(), "preparing release");

        self.inner.prepare(staged_release, destination).await
    }

    pub(crate) async fn release(
        &self,
        staged_release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        tracing::debug!(id =% staged_release.id, destination =% self.name(), "running release");
        self.inner.release(staged_release, destination).await
    }
}

#[async_trait::async_trait]
pub trait DestinationEdge {
    fn name(&self) -> DestinationIndex;

    async fn prepare(
        &self,
        release: &ReleaseItem,

        destination: &Destination,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    async fn release(&self, release: &ReleaseItem, destination: &Destination)
    -> anyhow::Result<()>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct DestinationIndex {
    pub organisation: String,
    pub name: String,
    pub version: usize,
}

impl Display for DestinationIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "{}/{}@{}",
            self.organisation, self.name, self.version
        ))
    }
}
