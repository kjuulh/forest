use std::collections::HashMap;

use anyhow::Context;
use non_models::{Destination, Namespace, ProjectName};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{State, services::artifact_staging_registry::ArtifactID};

pub struct ReleaseRegistry {
    db: PgPool,
}

impl ReleaseRegistry {
    #[allow(clippy::too_many_arguments)]
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
    ) -> anyhow::Result<ReleaseAnnotation> {
        let metadata = serde_json::to_value(metadata)?;
        let source = serde_json::to_value(source)?;
        let context = serde_json::to_value(context)?;
        let reference = serde_json::to_value(reference)?;

        let mut tx = self.db.begin().await.context("tx annotate")?;

        let project_id = sqlx::query!(
            "
                SELECT id
                FROM projects
                WHERE
                        namespace = $1
                    AND project = $2
                FOR UPDATE
            ",
            namespace,
            project
        )
        .fetch_optional(&mut *tx)
        .await
        .context("get project")?;

        let project_id = match project_id {
            Some(rec) => rec.id,
            None => {
                let rec = sqlx::query!(
                    "
                   INSERT INTO projects (
                       namespace,
                       project
                   ) VALUES (
                       $1,
                       $2
                   ) RETURNING id
                   ",
                    namespace,
                    project
                )
                .fetch_one(&mut *tx)
                .await
                .context("create project")?;

                rec.id
            }
        };

        let annotation_rec = sqlx::query!(
            "
                INSERT INTO annotations (
                    artifact_id,
                    slug,
                    metadata,
                    source,
                    context,
                    project_id,
                    ref
                ) VALUES (
                    $1,
                    $2,
                    $3,
                    $4,
                    $5,
                    $6,
                    $7
                )
                RETURNING id
            ",
            artifact_id,
            slug,
            metadata,
            source,
            context,
            project_id,
            reference,
        )
        .fetch_one(&mut *tx)
        .await
        .context("annotate (db)")?;

        tx.commit().await.context("annotate (db/tx)")?;

        Ok(ReleaseAnnotation {
            id: annotation_rec.id,
            artifact_id: *artifact_id,
            slug: slug.to_string(),
            metadata: serde_json::from_value(metadata).context("metadata")?,
            source: serde_json::from_value(source).context("source")?,
            context: serde_json::from_value(context).context("context")?,
            project: Project {
                namespace: namespace.to_string(),
                project: project.to_string(),
            },
        })
    }

    pub async fn release(
        &self,
        artifact_id: &ArtifactID,
        destinations: Vec<String>,
        environments: Vec<String>,
    ) -> anyhow::Result<()> {
        let annotation_rec = sqlx::query!(
            "
                SELECT id, project_id
                FROM annotations
                WHERE
                    artifact_id = $1
            ",
            artifact_id
        )
        .fetch_one(&self.db)
        .await
        .context("get annotation")?;

        let annotation_id = annotation_rec.id;
        let project_id = annotation_rec.project_id;

        let destination_ids = sqlx::query!(
            "SELECT DISTINCT id FROM destinations WHERE name = ANY($1) OR environment = ANY($2)",
            &destinations,
            &environments
        )
        .fetch_all(&self.db)
        .await
        .context("release")?;

        if destination_ids.len() < destinations.len() {
            anyhow::bail!("not all destinations exists")
        }

        if destination_ids.is_empty() {
            anyhow::bail!("found no destinations for requested environment");
        }

        let destination_ids: Vec<Uuid> = destination_ids.into_iter().map(|n| n.id).collect();

        // TODO: should likely be pushed to a leader, such that we have consistency in which thing is actually released and so on
        let mut tx = self.db.begin().await?;

        for destination_id in destination_ids {
            sqlx::query!(
                "
                INSERT INTO
                    releases (
                        artifact,
                        annotation_id,
                        project_id,
                        destination_id,
                        status
                    ) VALUES (
                        $1,
                        $2,
                        $3,
                        $4,
                        $5
                    )
                    ON CONFLICT (project_id, destination_id)
                    DO UPDATE SET
                        artifact = EXCLUDED.artifact,
                        annotation_id = EXCLUDED.annotation_id,
                        status = EXCLUDED.status,
                        updated = now()
                
            ",
                artifact_id,
                annotation_id,
                project_id,
                destination_id,
                "STAGED"
            )
            .execute(&mut *tx)
            .await
            .context(anyhow::anyhow!(
                "release: {} to {}",
                artifact_id,
                destination_id
            ))?;
        }

        tx.commit().await.context("commit release batch")?;

        Ok(())
    }

    pub async fn get_staged_release(
        &self,
    ) -> anyhow::Result<Option<(ReleaseItem, StagedReleaseTx)>> {
        let mut tx = self.db.begin().await?;

        let item = sqlx::query!(
            "
            SELECT
                id,
                artifact,
                annotation_id,
                project_id,
                destination_id,
                status
            FROM releases
            WHERE status = 'STAGED'
            LIMIT 1
            FOR UPDATE
            SKIP LOCKED
        "
        )
        .fetch_optional(&mut *tx)
        .await?;

        let Some(item) = item else {
            return Ok(None);
        };

        Ok(Some((
            ReleaseItem {
                id: item.id,
                artifact: item.artifact,
                project_id: item.project_id,
                destination_id: item.destination_id,
                status: item.status,
            },
            StagedReleaseTx { tx },
        )))
    }

    pub async fn commit_release_status(
        &self,
        release_item: &ReleaseItem,
        mut staged_release_tx: StagedReleaseTx,
        status: &str,
    ) -> anyhow::Result<()> {
        let res = sqlx::query!(
            "
                UPDATE releases
                SET
                    status = $2,
                    updated = now()
                WHERE id = $1
            ",
            release_item.id,
            status
        )
        .execute(&mut *staged_release_tx.tx)
        .await
        .context("update release status")?;

        if res.rows_affected() != 1 {
            anyhow::bail!(
                "setting release status failed to update row: {}",
                release_item.id
            );
        }

        tracing::debug!(release_id =% release_item.id, status, "committing final release status");
        staged_release_tx.tx.commit().await?;

        Ok(())
    }

    pub async fn get_release_annotation_by_slug(
        &self,
        slug: &str,
    ) -> anyhow::Result<ReleaseAnnotation> {
        let rec = sqlx::query!(
            "
                SELECT
                    a.id,
                    a.artifact_id,
                    a.slug,
                    a.metadata,
                    a.source,
                    a.context,
                    a.project_id,
                    p.namespace as namespace,
                    p.project as project
                FROM annotations a
                JOIN projects p ON a.project_id = p.id
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

    pub async fn get_namespaces(&self) -> anyhow::Result<Vec<Namespace>> {
        // TODO: consider if we should cursor this
        let recs = sqlx::query!(
            "
                SELECT DISTINCT namespace FROM projects;
            "
        )
        .fetch_all(&self.db)
        .await
        .context("get namespaces (db)")?;

        Ok(recs.into_iter().map(|r| r.namespace.into()).collect())
    }

    pub async fn get_projects_by_namespace(
        &self,
        namespace: &Namespace,
    ) -> anyhow::Result<Vec<ProjectName>> {
        // TODO: consider if we should cursor this
        let recs = sqlx::query!(
            "
                SELECT project
                FROM projects
                WHERE namespace = $1;
            ",
            namespace.as_str(),
        )
        .fetch_all(&self.db)
        .await
        .context("get projects (db)")?;

        Ok(recs.into_iter().map(|r| r.project.into()).collect())
    }

    pub async fn get_destinations(&self) -> anyhow::Result<Vec<Destination>> {
        // TODO: consider if we should cursor this
        let recs = sqlx::query!(
            "
                SELECT
                    id,
                    name,
                    metadata,
                    environment,
                    type_organisation,
                    type_name,
                    type_version
                FROM destinations 
            ",
        )
        .fetch_all(&self.db)
        .await
        .context("get destinations (db)")?;

        recs.into_iter()
            .map(|r| {
                Ok(Destination::new(
                    &r.name,
                    &r.environment,
                    serde_json::from_value(r.metadata).context("parse metadata")?,
                    non_models::DestinationType {
                        organisation: r.type_organisation,
                        name: r.type_name,
                        version: r.type_version as usize,
                    },
                ))
            })
            .collect::<anyhow::Result<Vec<_>>>()
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

pub struct ReleaseItem {
    pub id: Uuid,
    pub artifact: Uuid,
    pub project_id: Uuid,
    pub destination_id: Uuid,
    pub status: String,
}

pub struct StagedReleaseTx {
    tx: sqlx::Transaction<'static, sqlx::Postgres>,
}
