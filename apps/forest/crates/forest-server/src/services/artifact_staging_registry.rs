use std::{
    fmt::Display,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use sqlx::PgPool;

use crate::{actor::Actor, state::State};

#[derive(Clone)]
pub struct ArtifactStagingRegistry {
    db: PgPool,
    object_store: crate::object_store::ObjectStore,
}

impl ArtifactStagingRegistry {
    pub async fn create_staging_entry(&self, actor: &Actor) -> anyhow::Result<StagingArtifactID> {
        let id = StagingArtifactID::new();
        let actor_id = actor.actor_id();
        let actor_type = actor.actor_type();

        sqlx::query!(
            r#"
                INSERT INTO artifact_staging
                (
                    artifact_id,
                    actor_id,
                    actor_type
                )
                VALUES
                (
                    $1,
                    $2,
                    $3
                )
            "#,
            id.id(),
            actor_id,
            actor_type,
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
        category: &str,
    ) -> anyhow::Result<()> {
        // Store in S3
        let s3_key = crate::object_store::keys::artifact_file(
            &id.id().to_string(),
            env,
            destination,
            file_name,
        );
        self.object_store
            .put(&s3_key, file_content.as_bytes())
            .await
            .context("store artifact file in S3")?;

        // Also store in DB for backward compatibility during migration
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
        .await;

        let blob_id = match blob_entry {
            Ok(entry) => entry.id,
            Err(e) => {
                tracing::warn!("DB write failed after S3 upload, cleaning up: {e:#}");
                let _ = self.object_store.delete(&s3_key).await;
                return Err(e.into());
            }
        };

        let insert_result = sqlx::query!(
            r#"
                INSERT INTO artifact_files (
                    artifact_staging_id,
                    env,
                    destination,
                    file_name,
                    file_content,
                    category
                ) VALUES (
                    $1,
                    $2,
                    $3,
                    $4,
                    $5,
                    $6
                )
            "#,
            id.id(),
            env,
            destination,
            file_name,
            blob_id,
            category
        )
        .execute(&self.db)
        .await;

        if let Err(e) = insert_result {
            tracing::warn!("DB write failed after S3 upload, cleaning up: {e:#}");
            let _ = self.object_store.delete(&s3_key).await;
            return Err(e).context("create artifact file");
        }

        Ok(())
    }

    pub async fn get_files_for_release(
        &self,
        id: &uuid::Uuid,
        env: &str,
    ) -> anyhow::Result<Vec<(PathBuf, String)>> {
        let rec = sqlx::query!("SELECT artifact_id FROM artifacts WHERE id = $1", id)
            .fetch_one(&self.db)
            .await
            .context("get artifact id")?;
        let artifact_id = rec.artifact_id;

        // Get file metadata from DB
        let recs = sqlx::query!(
            "SELECT file_name, env, destination
             FROM artifact_files
             WHERE artifact_staging_id = $1 AND env = $2 AND category = 'deployment'",
            artifact_id,
            env
        )
        .fetch_all(&self.db)
        .await?;

        let mut result = Vec::new();
        for r in recs {
            let s3_key = crate::object_store::keys::artifact_file(
                &artifact_id.to_string(),
                &r.env,
                &r.destination,
                &r.file_name,
            );
            match self.object_store.get(&s3_key).await {
                Ok(content) => {
                    result.push((PathBuf::from(r.file_name), String::from_utf8_lossy(&content).to_string()));
                }
                Err(_) => {
                    // Fallback to DB for legacy data
                    let legacy = sqlx::query!(
                        "SELECT blob.content FROM artifact_files file
                         JOIN blob_storage blob ON file.file_content = blob.id
                         WHERE file.artifact_staging_id = $1 AND file.file_name = $2 AND file.env = $3 AND file.category = 'deployment'",
                        artifact_id, r.file_name, env
                    )
                    .fetch_optional(&self.db)
                    .await?;
                    if let Some(row) = legacy {
                        if let Some(content) = row.content {
                            result.push((PathBuf::from(r.file_name), content));
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    pub async fn get_spec_files(
        &self,
        id: &uuid::Uuid,
    ) -> anyhow::Result<Vec<(PathBuf, String)>> {
        let rec = sqlx::query!("SELECT artifact_id FROM artifacts WHERE id = $1", id)
            .fetch_one(&self.db)
            .await
            .context("get artifact id")?;
        let artifact_id = rec.artifact_id;

        let recs = sqlx::query!(
            "SELECT file_name, env, destination
             FROM artifact_files
             WHERE artifact_staging_id = $1 AND category = 'spec'",
            artifact_id
        )
        .fetch_all(&self.db)
        .await?;

        let mut result = Vec::new();
        for r in recs {
            let s3_key = crate::object_store::keys::artifact_file(
                &artifact_id.to_string(),
                &r.env,
                &r.destination,
                &r.file_name,
            );
            match self.object_store.get(&s3_key).await {
                Ok(content) => {
                    result.push((PathBuf::from(r.file_name), String::from_utf8_lossy(&content).to_string()));
                }
                Err(_) => {
                    // Fallback to DB for legacy data
                    let legacy = sqlx::query!(
                        "SELECT blob.content FROM artifact_files file
                         JOIN blob_storage blob ON file.file_content = blob.id
                         WHERE file.artifact_staging_id = $1 AND file.file_name = $2 AND file.category = 'spec'",
                        artifact_id, r.file_name
                    )
                    .fetch_optional(&self.db)
                    .await?;
                    if let Some(row) = legacy {
                        if let Some(content) = row.content {
                            result.push((PathBuf::from(r.file_name), content));
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    pub async fn get_artifact_files(
        &self,
        artifact_id: &uuid::Uuid,
        category: Option<&str>,
    ) -> anyhow::Result<Vec<ArtifactFileEntry>> {
        let rec = sqlx::query!("SELECT artifact_id FROM artifacts WHERE id = $1", artifact_id)
            .fetch_one(&self.db)
            .await
            .context("get artifact id")?;
        let staging_id = rec.artifact_id;

        // Get metadata from DB, content from S3
        let recs = sqlx::query!(
            r#"
                SELECT file_name, category, env, destination
                FROM artifact_files
                WHERE artifact_staging_id = $1
                  AND ($2::text IS NULL OR category = $2)
                ORDER BY category, file_name
            "#,
            staging_id,
            category,
        )
        .fetch_all(&self.db)
        .await
        .context("get artifact file metadata")?;

        let mut entries = Vec::new();
        for r in recs {
            let s3_key = crate::object_store::keys::artifact_file(
                &staging_id.to_string(),
                &r.env,
                &r.destination,
                &r.file_name,
            );
            let content = match self.object_store.get(&s3_key).await {
                Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                Err(_) => {
                    // Fallback to DB for legacy data
                    let legacy = sqlx::query!(
                        "SELECT blob.content FROM artifact_files file
                         JOIN blob_storage blob ON file.file_content = blob.id
                         WHERE file.artifact_staging_id = $1 AND file.file_name = $2",
                        staging_id, r.file_name
                    )
                    .fetch_optional(&self.db)
                    .await?
                    .and_then(|row| row.content)
                    .unwrap_or_default();
                    legacy
                }
            };

            entries.push(ArtifactFileEntry {
                file_name: r.file_name,
                category: r.category,
                env: r.env,
                destination: r.destination,
                content,
            });
        }
        Ok(entries)
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

pub struct ArtifactFileEntry {
    pub file_name: String,
    pub category: String,
    pub env: String,
    pub destination: String,
    pub content: String,
}

pub struct StagingArtifactID {
    id: uuid::Uuid,
    created: SystemTime,
}

impl Default for StagingArtifactID {
    fn default() -> Self {
        Self::new()
    }
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
            object_store: self.object_store.clone(),
        }
    }
}
