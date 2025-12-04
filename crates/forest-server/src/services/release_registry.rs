use std::collections::HashMap;

use anyhow::Context;
use forest_models::{Destination, Namespace, ProjectName, ReleaseStatus};
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
                RETURNING id, created
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
            destinations: Vec::new(),
            created_at: annotation_rec.created,
        })
    }

    pub async fn release(
        &self,
        artifact_id: &ArtifactID,
        destinations: Vec<String>,
        environments: Vec<String>,
    ) -> anyhow::Result<CreatedReleaseIntent> {
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

        let destination_recs = sqlx::query!(
            "SELECT DISTINCT id, name, environment FROM destinations WHERE name = ANY($1) OR environment = ANY($2)",
            &destinations,
            &environments
        )
        .fetch_all(&self.db)
        .await
        .context("release")?;

        if destination_recs.len() < destinations.len() {
            anyhow::bail!("not all destinations exists")
        }

        if destination_recs.is_empty() {
            anyhow::bail!("found no destinations for requested environment");
        }

        // TODO: should likely be pushed to a leader, such that we have consistency in which thing is actually released and so on
        let mut tx = self.db.begin().await?;

        // 1. Create ONE release_intent for this release request
        let release_intent = sqlx::query!(
            "
            INSERT INTO release_intents (
                artifact,
                annotation_id,
                project_id
            ) VALUES (
                $1,
                $2,
                $3
            )
            RETURNING id
            ",
            artifact_id,
            annotation_id,
            project_id
        )
        .fetch_one(&mut *tx)
        .await
        .context("create release_intent")?;

        let mut created_releases = Vec::new();

        // 2. Create/update releases for each destination, all pointing to this intent
        for dest in &destination_recs {
            sqlx::query!(
                "
                INSERT INTO releases (
                    release_intent_id,
                    project_id,
                    destination_id,
                    status
                ) VALUES (
                    $1,
                    $2,
                    $3,
                    $4
                )
                ON CONFLICT (project_id, destination_id)
                DO UPDATE SET
                    release_intent_id = EXCLUDED.release_intent_id,
                    status = EXCLUDED.status,
                    updated = now()
                ",
                release_intent.id,
                project_id,
                dest.id,
                "STAGED"
            )
            .execute(&mut *tx)
            .await
            .context(anyhow::anyhow!(
                "upsert release: {} to {}",
                artifact_id,
                dest.id
            ))?;

            created_releases.push(CreatedRelease {
                destination: dest.name.clone(),
                environment: dest.environment.clone(),
            });
        }

        tx.commit().await.context("commit release batch")?;

        Ok(CreatedReleaseIntent {
            release_intent_id: release_intent.id,
            releases: created_releases,
        })
    }

    /// Get release status by release_intent_id
    /// Returns all releases (destinations) for this intent with their statuses
    pub async fn get_release_status_by_intent(
        &self,
        release_intent_id: &Uuid,
    ) -> anyhow::Result<Vec<ReleaseStatusInfo>> {
        let records = sqlx::query!(
            r#"
                SELECT
                    r.id as release_id,
                    r.release_intent_id,
                    r.destination_id,
                    r.status,
                    d.name as destination
                FROM releases r
                JOIN destinations d ON r.destination_id = d.id
                WHERE r.release_intent_id = $1
            "#,
            release_intent_id
        )
        .fetch_all(&self.db)
        .await
        .context("get_release_status_by_intent")?;

        records
            .into_iter()
            .map(|record| {
                let status: ReleaseStatus = record
                    .status
                    .parse()
                    .map_err(|e| anyhow::anyhow!("{}", e))?;

                Ok(ReleaseStatusInfo {
                    release_id: record.release_id,
                    release_intent_id: record.release_intent_id,
                    destination_id: record.destination_id,
                    destination: record.destination,
                    status,
                })
            })
            .collect()
    }

    pub async fn get_staged_release(
        &self,
    ) -> anyhow::Result<Option<(ReleaseItem, StagedReleaseTx)>> {
        let mut tx = self.db.begin().await?;

        // Pick up staged releases (status is now on releases table)
        let item = sqlx::query!(
            "
            SELECT
                r.id as release_id,
                r.release_intent_id,
                r.destination_id,
                r.status,
                ri.artifact,
                ri.project_id
            FROM releases r
            JOIN release_intents ri ON r.release_intent_id = ri.id
            WHERE r.status = 'STAGED'
            LIMIT 1
            FOR UPDATE OF r
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
                id: item.release_id,
                release_intent_id: item.release_intent_id,
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
        status: ReleaseStatus,
    ) -> anyhow::Result<()> {
        let status_str = status.as_str();
        // Update the release status (status is on releases table, per destination)
        let res = sqlx::query!(
            "
                UPDATE releases
                SET
                    status = $2,
                    updated = now()
                WHERE id = $1
            ",
            release_item.id,
            status_str
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

        tracing::debug!(
            release_id =% release_item.id,
            release_intent_id =% release_item.release_intent_id,
            %status,
            "committing final release status"
        );
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
                    a.created,
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
            destinations: Vec::new(),
            created_at: rec.created,
        })
    }

    pub async fn get_release_annotation_by_project(
        &self,
        namespace: &str,
        project: &str,
    ) -> anyhow::Result<Vec<ReleaseAnnotation>> {
        // Use LEFT JOINs to get annotations even if they have no releases/destinations
        // This query may return multiple rows per annotation (one per destination)
        // Join through release_intents to find which destinations have this annotation released
        let recs = sqlx::query!(
            r#"
                SELECT
                    a.id                 as id,
                    a.artifact_id        as artifact_id,
                    a.slug               as slug,
                    a.metadata           as metadata,
                    a.source             as source,
                    a.context            as context,
                    a.project_id         as project_id,
                    a.created            as created,
                    p.namespace          as namespace,
                    p.project            as project,
                    d.environment        as "environment?",
                    d.name               as "destination_name?",
                    d.type_organisation  as "destination_type?",
                    d.type_name          as "destination_type_name?",
                    d.type_version       as "destination_type_version?"
                FROM annotations a
                JOIN projects p ON a.project_id = p.id
                LEFT JOIN release_intents ri ON a.id = ri.annotation_id
                LEFT JOIN releases r ON r.release_intent_id = ri.id
                LEFT JOIN destinations d ON d.id = r.destination_id
                WHERE
                        p.namespace = $1
                    AND p.project = $2
                ORDER BY a.created DESC, a.id
            "#,
            namespace,
            project
        )
        .fetch_all(&self.db)
        .await
        .context("get annotations (db)")?;

        // Group results by annotation ID to consolidate destinations
        // Use IndexMap to preserve insertion order (sorted by created DESC)
        let mut annotations_map: indexmap::IndexMap<Uuid, ReleaseAnnotation> =
            indexmap::IndexMap::new();

        for rec in recs {
            let annotation_id = rec.id;

            // Build destination if present
            let destination = match (
                rec.destination_name,
                rec.environment,
                rec.destination_type,
                rec.destination_type_name,
                rec.destination_type_version,
            ) {
                (Some(name), Some(env), Some(org), Some(type_name), Some(version)) => {
                    Some(ReleaseDestination {
                        name,
                        environment: env,
                        type_organisation: org,
                        type_name,
                        type_version: version,
                    })
                }
                _ => None,
            };

            if let Some(annotation) = annotations_map.get_mut(&annotation_id) {
                // Annotation already exists, just add the destination if present
                if let Some(dest) = destination {
                    annotation.destinations.push(dest);
                }
            } else {
                // Create new annotation entry
                let mut destinations = Vec::new();
                if let Some(dest) = destination {
                    destinations.push(dest);
                }

                let annotation = ReleaseAnnotation {
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
                    destinations,
                    created_at: rec.created,
                };

                annotations_map.insert(annotation_id, annotation);
            }
        }

        Ok(annotations_map.into_values().collect())
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
                    forest_models::DestinationType {
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
    pub destinations: Vec<ReleaseDestination>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct ReleaseDestination {
    pub name: String,
    pub environment: String,
    pub type_organisation: String,
    pub type_name: String,
    pub type_version: i32,
}

#[derive(Clone)]
pub struct ReleaseItem {
    pub id: Uuid,
    pub release_intent_id: Uuid,
    pub artifact: Uuid,
    pub project_id: Uuid,
    pub destination_id: Uuid,
    pub status: String,
}

pub struct ReleaseStatusInfo {
    pub release_id: Uuid,
    pub release_intent_id: Uuid,
    pub destination_id: Uuid,
    pub destination: String,
    pub status: ReleaseStatus,
}

pub struct CreatedReleaseIntent {
    pub release_intent_id: Uuid,
    pub releases: Vec<CreatedRelease>,
}

pub struct CreatedRelease {
    pub destination: String,
    pub environment: String,
}

pub struct StagedReleaseTx {
    tx: sqlx::Transaction<'static, sqlx::Postgres>,
}
