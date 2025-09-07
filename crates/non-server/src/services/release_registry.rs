use std::collections::HashMap;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{State, services::artifact_staging_registry::ArtifactID};

pub struct ReleaseRegistry {
    db: PgPool,
}

impl ReleaseRegistry {
    pub async fn annotate(
        &self,
        artifact_id: &ArtifactID,
        slug: &str,
        metadata: &HashMap<String, String>,
        source: &Source,
        context: &ArtifactContext,
        namespace: &str,
        project: &str,
        reference: &Reference,
    ) -> anyhow::Result<()> {
        let metadata = serde_json::to_value(metadata)?;
        let source = serde_json::to_value(source)?;
        let context = serde_json::to_value(context)?;
        let reference = serde_json::to_value(reference)?;

        sqlx::query!(
            "
                INSERT INTO annotations (
                    artifact_id,
                    slug,
                    metadata,
                    source,
                    context,
                    namespace,
                    project,
                    ref
                ) VALUES (
                    $1,
                    $2,
                    $3,
                    $4,
                    $5,
                    $6,
                    $7,
                    $8
                )
            ",
            artifact_id,
            slug,
            metadata,
            source,
            context,
            namespace,
            project,
            reference,
        )
        .execute(&self.db)
        .await
        .context("annotate (db)")?;

        Ok(())
    }

    pub async fn release(
        &self,
        artifact_id: &ArtifactID,
        destinations: Vec<String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn get_release_annotation_by_slug(
        &self,
        slug: &str,
    ) -> anyhow::Result<ReleaseAnnotation> {
        let rec = sqlx::query!(
            "
                SELECT
                    id,
                    artifact_id,
                    slug,
                    metadata,
                    source,
                    context,
                    namespace,
                    project
                FROM annotations
                WHERE
                    slug = $1
            ",
            slug
        )
        .fetch_optional(&self.db)
        .await
        .context("get annotation (db)")?;

        let Some(rec) = rec else {
            anyhow::bail!("failed to find annotation with slug: {}", slug);
        };

        Ok(ReleaseAnnotation {
            id: rec.id,
            artifact_id: rec.artifact_id,
            slug: rec.slug,
            metadata: serde_json::from_value(rec.metadata).context("metadata")?,
            source: serde_json::from_value(rec.source).context("source")?,
            context: serde_json::from_value(rec.context).context("context")?,
            project: Project {
                namespace: rec.namespace,
                project: rec.project,
            },
        })
    }
}

pub trait ReleaseRegistryState {
    fn release_registry(&self) -> ReleaseRegistry;
}

impl ReleaseRegistryState for State {
    fn release_registry(&self) -> ReleaseRegistry {
        ReleaseRegistry {
            db: self.db.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Source {
    pub username: Option<String>,
    pub email: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct ArtifactContext {
    pub title: String,
    pub description: Option<String>,
    pub web: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Reference {
    pub commit_sha: String,
    pub commit_branch: Option<String>,
}

pub struct Project {
    pub namespace: String,
    pub project: String,
}

pub struct ReleaseAnnotation {
    pub id: Uuid,
    pub artifact_id: Uuid,
    pub slug: String,
    pub metadata: HashMap<String, String>,
    pub source: Source,
    pub context: ArtifactContext,
    pub project: Project,
}
