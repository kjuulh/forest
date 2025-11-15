use std::{
    fmt::Display,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use sqlx::PgPool;

use crate::state::State;

pub struct ArtifactStagingRegistry {
    db: PgPool,
}

impl ArtifactStagingRegistry {
    pub async fn create_staging_entry(&self) -> anyhow::Result<StagingArtifactID> {
        let id = StagingArtifactID::new();

        sqlx::query!(
            r#"
                INSERT INTO artifact_staging
                (
                    artifact_id
                )
                VALUES
                (
                    $1
                )
            "#,
            id.id()
        )
        .execute(&self.db)
        .await
        .context("create staging entry")?;

        Ok(id)
    }

    pub async fn upload_file(
        &self,
        id: &StagingArtifactID,
        file_name: &str,
        file_content: &str,
        env: &str,
        destination: &str,
    ) -> anyhow::Result<()> {
        let blob_entry = sqlx::query!(
            r#"
                INSERT INTO blob_storage (
                    content
                ) VALUES (
                    $1
                ) RETURNING id
            "#,
            file_content
        )
        .fetch_one(&self.db)
        .await?;

        let blob_id = blob_entry.id;

        sqlx::query!(
            r#"
                INSERT INTO artifact_files (
                    artifact_staging_id,
                    env,
                    destination,
                    file_name,
                    file_content
                ) VALUES (
                    $1,
                    $2,
                    $3,
                    $4,
                    $5
                )
            "#,
            id.id(),
            env,
            destination,
            file_name,
            blob_id
        )
        .execute(&self.db)
        .await
        .context("create artifact file")?;

        Ok(())
    }

    pub async fn commit_staging(&self, id: &StagingArtifactID) -> anyhow::Result<ArtifactID> {
        let rec = sqlx::query!(
            "
                INSERT INTO artifacts (
                    artifact_id
                ) VALUES (
                    $1
                ) RETURNING id
            ",
            id.id()
        )
        .fetch_one(&self.db)
        .await
        .context("failed to commit artifact")?;

        Ok(rec.id)
    }
}

pub struct StagingArtifactID {
    id: uuid::Uuid,
    created: SystemTime,
}

impl StagingArtifactID {
    pub fn new() -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            created: std::time::SystemTime::now(),
        }
    }

    pub fn created(&self) -> &SystemTime {
        &self.created
    }

    pub fn id(&self) -> &uuid::Uuid {
        &self.id
    }
}

impl TryFrom<String> for StagingArtifactID {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.as_str().try_into()
    }
}

impl TryFrom<&str> for StagingArtifactID {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let Some((start, end)) = value.split_once(".") else {
            anyhow::bail!("id contains no '.' seperator")
        };

        Ok(Self {
            id: end
                .parse::<uuid::Uuid>()
                .context("failed to parsed id as uuid (v4)")?,
            created: SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_secs(
                    start
                        .parse::<u64>()
                        .context("failed to parse timestamp as unsigned int 64")?,
                ))
                .context("time is not valid")?,
        })
    }
}

impl Display for StagingArtifactID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "{}.{}",
            self.created
                .duration_since(UNIX_EPOCH)
                .expect("to be able to get timestamp")
                .as_secs(),
            self.id
        ))
    }
}

pub type ArtifactID = uuid::Uuid;

pub trait ArtifactStagingRegistryState {
    fn artifact_staging_registry(&self) -> ArtifactStagingRegistry;
}

impl ArtifactStagingRegistryState for State {
    fn artifact_staging_registry(&self) -> ArtifactStagingRegistry {
        ArtifactStagingRegistry {
            db: self.db.clone(),
        }
    }
}
