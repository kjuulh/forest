use std::{fmt::Display, sync::Arc};

use non_models::Destination;
use sqlx::PgPool;

use crate::{
    State,
    destinations::{
        kubernetesv1::KubernetesV1Destination, logger::DestinationLogger,
        terraformv1::TerraformV1Destination,
    },
    services::{
        artifact_staging_registry::ArtifactStagingRegistryState, release_registry::ReleaseItem,
    },
    temp_dir::TempDirectoriesState,
};

pub mod kubernetesv1;
pub mod terraformv1;

pub mod logger;

pub struct DestinationService {
    inner: Arc<dyn DestinationEdge + Send + Sync + 'static>,
    db: PgPool,
}

impl DestinationService {
    pub fn new<T: DestinationEdge + Send + Sync + 'static>(t: T, db: PgPool) -> Self {
        Self {
            inner: Arc::new(t),
            db,
        }
    }

    pub fn new_kubernetes_v1(db: PgPool) -> Self {
        Self::new(KubernetesV1Destination {}, db)
    }

    pub fn new_terraform_v1(state: &State, db: PgPool) -> Self {
        Self::new(
            TerraformV1Destination {
                temp: state.temp_directories(),
                artifact_files: state.artifact_staging_registry(),
            },
            db,
        )
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

        let logger = self.create_logger(&staged_release);

        self.inner
            .prepare(&logger, staged_release, destination)
            .await
    }

    pub(crate) async fn release(
        &self,
        staged_release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        tracing::debug!(id =% staged_release.id, destination =% self.name(), "running release");
        let logger = self.create_logger(&staged_release);

        self.inner
            .release(&logger, staged_release, destination)
            .await
    }

    fn create_logger(&self, staged_release: &ReleaseItem) -> DestinationLogger {
        DestinationLogger::new(staged_release.clone(), self.db.clone())
    }
}

#[async_trait::async_trait]
pub trait DestinationEdge {
    fn name(&self) -> DestinationIndex;

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
