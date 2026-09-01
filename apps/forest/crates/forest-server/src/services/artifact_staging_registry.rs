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
        // Keyed by the file's own path, not by env/destination — so the server
        // stays free to correct those columns later without moving the object.
        // See `keys::artifact_file_by_name`.
        let s3_key =
            crate::object_store::keys::artifact_file_by_name(&id.id().to_string(), file_name);
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
            if let Some(content) = self
                .file_content(&artifact_id, &r.env, &r.destination, &r.file_name)
                .await
            {
                result.push((PathBuf::from(r.file_name), content));
            }
        }
        Ok(result)
    }

    pub async fn get_spec_files(&self, id: &uuid::Uuid) -> anyhow::Result<Vec<(PathBuf, String)>> {
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
            if let Some(content) = self
                .file_content(&artifact_id, &r.env, &r.destination, &r.file_name)
                .await
            {
                result.push((PathBuf::from(r.file_name), content));
            }
        }
        Ok(result)
    }

    pub async fn get_artifact_files(
        &self,
        artifact_id: &uuid::Uuid,
        category: Option<&str>,
    ) -> anyhow::Result<Vec<ArtifactFileEntry>> {
        let rec = sqlx::query!(
            "SELECT artifact_id FROM artifacts WHERE id = $1",
            artifact_id
        )
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
            let content = self
                .file_content(&staging_id, &r.env, &r.destination, &r.file_name)
                .await
                .unwrap_or_default();

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

    /// Freeze a staged upload into an artifact.
    ///
    /// Before doing so, re-derive which deployment item each uploaded file
    /// belongs to and correct `env`/`destination` from the tree itself.
    ///
    /// The client sends those two values per file, and it used to be the client
    /// that decided them — by splitting the upload path on `/`, in two
    /// copy-pasted loops, when both the selector and the destination type
    /// routinely contain `/`. So every client truncated the selector, and fixing
    /// one copy left the other wrong. The server has the whole tree and each
    /// item's own record of what it is, so it decides here instead: one
    /// implementation, and every client is correct including the ones nobody
    /// upgrades.
    ///
    /// A file the tree cannot place keeps whatever it was uploaded with — this
    /// corrects what it can prove and touches nothing else.
    pub async fn commit_staging(&self, id: &StagingArtifactID) -> anyhow::Result<ArtifactID> {
        if let Err(e) = self.reattribute_deployment_files(id).await {
            // Refuse the commit: a wrongly-attributed file is a release aimed at
            // the wrong place, and this is the last point at which anyone is
            // still watching.
            return Err(e.context("attribute this artifact's deployment files"));
        }

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

impl ArtifactStagingRegistry {
    /// An artifact file's content, wherever it happens to live.
    ///
    /// Three places, newest first: the current key, the pre-correction key that
    /// mixed env and destination in, and the `blob_storage` copy `upload_file`
    /// writes alongside. Older artifacts genuinely are in the older places, and
    /// a release of one must not start failing because the key shape moved.
    async fn file_content(
        &self,
        staging_id: &uuid::Uuid,
        env: &str,
        destination: &str,
        file_name: &str,
    ) -> Option<String> {
        let id = staging_id.to_string();

        let current = crate::object_store::keys::artifact_file_by_name(&id, file_name);
        if let Ok(bytes) = self.object_store.get(&current).await {
            return Some(String::from_utf8_lossy(&bytes).to_string());
        }

        let legacy = crate::object_store::keys::artifact_file(&id, env, destination, file_name);
        if let Ok(bytes) = self.object_store.get(&legacy).await {
            return Some(String::from_utf8_lossy(&bytes).to_string());
        }

        sqlx::query_scalar!(
            "SELECT blob.content
             FROM artifact_files file
             JOIN blob_storage blob ON file.file_content = blob.id
             WHERE file.artifact_staging_id = $1 AND file.file_name = $2",
            staging_id,
            file_name,
        )
        .fetch_optional(&self.db)
        .await
        .ok()
        .flatten()
        .flatten()
    }
}

impl ArtifactStagingRegistry {
    /// Correct `artifact_files.env`/`.destination` for deployment files from the
    /// item records in the uploaded tree. See `commit_staging`.
    async fn reattribute_deployment_files(&self, id: &StagingArtifactID) -> anyhow::Result<()> {
        use crate::services::destination_selector;

        let rows = sqlx::query!(
            r#"SELECT f.file_name, f.env, f.destination, blob.content
               FROM artifact_files f
               JOIN blob_storage blob ON blob.id = f.file_content
               WHERE f.artifact_staging_id = $1 AND f.category = 'deployment'"#,
            id.id(),
        )
        .fetch_all(&self.db)
        .await
        .context("read the staged deployment files")?;

        if rows.is_empty() {
            return Ok(());
        }

        let files: Vec<(PathBuf, String)> = rows
            .iter()
            .map(|r| {
                (
                    PathBuf::from(&r.file_name),
                    r.content.clone().unwrap_or_default(),
                )
            })
            .collect();

        let items = destination_selector::parse_items(&files)?;
        if items.is_empty() {
            return Ok(());
        }

        let current: std::collections::HashMap<&str, (&str, &str)> = rows
            .iter()
            .map(|r| {
                (
                    r.file_name.as_str(),
                    (r.env.as_str(), r.destination.as_str()),
                )
            })
            .collect();

        for (file_name, env, destination) in destination_selector::attribute_files(&files, &items) {
            if current.get(file_name) == Some(&(env.as_str(), destination.as_str())) {
                continue;
            }

            sqlx::query!(
                "UPDATE artifact_files SET env = $3, destination = $4, updated = now()
                 WHERE artifact_staging_id = $1 AND file_name = $2 AND category = 'deployment'",
                id.id(),
                file_name,
                env,
                destination,
            )
            .execute(&self.db)
            .await
            .context("correct a deployment file's env/destination")?;

            tracing::debug!(
                file_name,
                env,
                destination,
                "corrected a deployment file's attribution from its item record"
            );
        }

        Ok(())
    }
}
