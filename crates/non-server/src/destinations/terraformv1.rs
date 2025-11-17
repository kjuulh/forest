use anyhow::Context;
use non_models::Destination;

use crate::{
    destinations::{DestinationEdge, DestinationIndex},
    services::{artifact_staging_registry::ArtifactStagingRegistry, release_registry::ReleaseItem},
    temp_dir::TempDirectories,
};

pub struct TerraformV1Destination {
    pub temp: TempDirectories,
    pub artifact_files: ArtifactStagingRegistry,
}

impl TerraformV1Destination {
    pub async fn run(
        &self,
        release: &ReleaseItem,
        destination: &Destination,
        mode: Mode,
    ) -> anyhow::Result<()> {
        let temp_dir = self.temp.create_emphemeral_temp().await?;
        let files = self
            .artifact_files
            .get_files_for_release(&release.artifact, &destination.environment)
            .await
            .context("get files for release")?;

        // 1. Fill temp dir with the correct files
        for (path, content) in files {
            let path = temp_dir.join(path);
            tracing::debug!("placing files in: {}", path.display());
        }

        // 2. Run terraform command over it

        Ok(())
    }
}

#[async_trait::async_trait]
impl DestinationEdge for TerraformV1Destination {
    fn name(&self) -> DestinationIndex {
        DestinationIndex {
            organisation: "non".into(),
            name: "terraform".into(),
            version: 1,
        }
    }
    async fn prepare(
        &self,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        self.run(release, destination, Mode::Prepare)
            .await
            .context("terraform plan failed")?;

        Ok(())
    }

    async fn release(
        &self,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        self.run(release, destination, Mode::Apply)
            .await
            .context("terraform plan failed")?;

        Ok(())
    }
}

enum Mode {
    Prepare,
    Apply,
}
