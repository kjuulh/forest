use std::collections::HashMap;

use anyhow::Context;
use forest_models::Destination;
use forest_runner::destinations::fluxv1::{FluxV1Handler, Mode};
use sqlx::PgPool;

use crate::{
    destinations::{DestinationEdge, DestinationIndex, logger::DestinationLogger},
    services::{
        artifact_staging_registry::ArtifactStagingRegistry, release_registry::ReleaseItem,
    },
    temp_dir::TempDirectories,
};

use super::in_process_backend::InProcessBackend;

/// Flux v2 GitOps destination — thin adapter that delegates to
/// `FluxV1Handler` from the `forest-runner` crate via an `InProcessBackend`.
pub struct FluxV1Destination {
    pub temp: TempDirectories,
    pub artifact_files: ArtifactStagingRegistry,
    pub db: PgPool,
}

impl FluxV1Destination {
    fn create_backend(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> InProcessBackend {
        InProcessBackend::new(
            self.artifact_files.clone(),
            self.db.clone(),
            logger.clone(),
            self.temp.clone(),
            release.artifact,
            release.project_id,
            destination.environment.clone(),
        )
    }
}

#[async_trait::async_trait]
impl DestinationEdge for FluxV1Destination {
    fn name(&self) -> DestinationIndex {
        DestinationIndex {
            organisation: "forest".into(),
            name: "flux".into(),
            version: 1,
        }
    }

    fn validate_metadata(&self, metadata: &HashMap<String, String>) -> anyhow::Result<()> {
        FluxV1Handler::validate_metadata(metadata)
    }

    async fn prepare(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        let backend = self.create_backend(logger, release, destination);
        let config = InProcessBackend::config_from_destination(destination);
        FluxV1Handler::run(&backend, &config, Mode::Prepare)
            .await
            .context("flux prepare failed")
    }

    async fn release(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        let backend = self.create_backend(logger, release, destination);
        let config = InProcessBackend::config_from_destination(destination);
        FluxV1Handler::run(&backend, &config, Mode::Apply)
            .await
            .context("flux release failed")
    }
}
