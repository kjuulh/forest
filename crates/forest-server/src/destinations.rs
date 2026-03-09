use std::{collections::HashMap, fmt::Display, sync::Arc};

use forest_models::Destination;

use crate::{
    State,
    destinations::{
        fluxv1::FluxV1Destination,
        foragev1::ForageV1Destination,
        kubernetesv1::KubernetesV1Destination,
        logger::DestinationLogger,
        terraformv1::{TerraformStateStoreState, TerraformV1Destination},
    },
    services::{
        artifact_staging_registry::ArtifactStagingRegistryState,
        release_logs_registry::ReleaseLogsRegistry, release_registry::ReleaseItem,
    },
    temp_dir::TempDirectoriesState,
};

pub mod fluxv1;
pub mod foragev1;
pub mod in_process_backend;
pub mod kubernetesv1;
pub mod terraformv1;

pub mod logger;

pub struct DestinationService {
    inner: Arc<dyn DestinationEdge + Send + Sync + 'static>,
    release_logs_registry: ReleaseLogsRegistry,
}

impl DestinationService {
    pub fn new<T: DestinationEdge + Send + Sync + 'static>(
        t: T,
        release_logs_registry: ReleaseLogsRegistry,
    ) -> Self {
        Self {
            inner: Arc::new(t),
            release_logs_registry,
        }
    }

    pub fn new_flux_v1(state: &State, release_logs_registry: ReleaseLogsRegistry) -> Self {
        Self::new(
            FluxV1Destination {
                temp: state.temp_directories(),
                artifact_files: state.artifact_staging_registry(),
                db: state.db.clone(),
            },
            release_logs_registry,
        )
    }

    pub fn new_kubernetes_v1(release_logs_registry: ReleaseLogsRegistry) -> Self {
        Self::new(KubernetesV1Destination {}, release_logs_registry)
    }

    pub fn new_forage_v1(release_logs_registry: ReleaseLogsRegistry) -> Self {
        Self::new(ForageV1Destination {}, release_logs_registry)
    }

    pub fn new_terraform_v1(state: &State, release_logs_registry: ReleaseLogsRegistry) -> Self {
        Self::new(
            TerraformV1Destination {
                temp: state.temp_directories(),
                artifact_files: state.artifact_staging_registry(),
                tf_state: state.terraform_state_store(),
            },
            release_logs_registry,
        )
    }

    #[inline(always)]
    pub fn name(&self) -> DestinationIndex {
        self.inner.name()
    }

    pub fn validate_metadata(&self, metadata: &HashMap<String, String>) -> anyhow::Result<()> {
        self.inner.validate_metadata(metadata)
    }

    pub(crate) async fn prepare(
        &self,
        logger: &DestinationLogger,
        staged_release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        tracing::debug!(id =% staged_release.id, destination =% self.name(), "preparing release");

        self.inner
            .prepare(logger, staged_release, destination)
            .await
    }

    pub(crate) async fn release(
        &self,
        logger: &DestinationLogger,
        staged_release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        tracing::debug!(id =% staged_release.id, destination =% self.name(), "running release");

        self.inner
            .release(logger, staged_release, destination)
            .await
    }

    fn create_logger(&self, staged_release: &ReleaseItem) -> DestinationLogger {
        DestinationLogger::new(staged_release.clone(), self.release_logs_registry.clone())
    }
}

#[async_trait::async_trait]
pub trait DestinationEdge {
    fn name(&self) -> DestinationIndex;

    /// Validate that the given metadata contains all required fields for this
    /// destination type. Called during destination creation.
    #[allow(unused_variables)]
    fn validate_metadata(&self, metadata: &HashMap<String, String>) -> anyhow::Result<()> {
        Ok(())
    }

    #[allow(unused_variables)]
    async fn prepare(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    async fn release(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()>;
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
