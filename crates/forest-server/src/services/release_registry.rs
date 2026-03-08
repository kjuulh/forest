use std::collections::HashMap;

use crate::repositories::error::DbError;
use anyhow::Context;
use forest_models::{Destination, OrganisationName, ProjectName, ReleaseStatus};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{State, actor::Actor, services::artifact_staging_registry::ArtifactID};

#[derive(Clone)]
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
        organisation: &str,
        project: &str,
        reference: &Reference,
        actor: &Actor,
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
                        organisation = $1
                    AND project = $2
                FOR UPDATE
            ",
            organisation,
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
                       organisation,
                       project
                   ) VALUES (
                       $1,
                       $2
                   ) RETURNING id
                   ",
                    organisation,
                    project
                )
                .fetch_one(&mut *tx)
                .await
                .map_err(DbError::from)?;

                rec.id
            }
        };

        let actor_id = actor.actor_id();
        let actor_type = actor.actor_type();

        let annotation_rec = sqlx::query!(
            "
                INSERT INTO annotations (
                    artifact_id,
                    slug,
                    metadata,
                    source,
                    context,
                    project_id,
                    ref,
                    actor_id,
                    actor_type
                ) VALUES (
                    $1,
                    $2,
                    $3,
                    $4,
                    $5,
                    $6,
                    $7,
                    $8,
                    $9
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
            actor_id,
            actor_type,
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
                organisation: organisation.to_string(),
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
        actor: &Actor,
        event_store: &crate::services::release_event_store::ReleaseEventStore,
        force: bool,
        use_pipeline: bool,
        pipeline_registry: &crate::services::release_pipeline::ReleasePipelineRegistry,
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

        let (organisation, project) = self.get_project_context(&project_id).await?;

        // Pipeline mode: only when explicitly requested
        if use_pipeline {
            let pipeline = pipeline_registry
                .get_enabled_for_project(&project_id)
                .await
                .context("check for pipeline")?
                .context("no enabled release pipeline found for this project")?;

            return self
                .release_with_pipeline(
                    artifact_id,
                    &annotation_id,
                    &project_id,
                    &organisation,
                    &project,
                    actor,
                    event_store,
                    force,
                    pipeline,
                )
                .await;
        }

        // Non-pipeline mode: flat release to all requested destinations
        let destination_recs = sqlx::query!(
            r#"
            SELECT DISTINCT d.id, d.name, d.environment
            FROM destinations d
            LEFT JOIN environments e ON d.environment_id = e.id
            WHERE d.name = ANY($1) OR e.name = ANY($2)
            "#,
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

        let actor_id = actor.actor_id();
        let actor_type = actor.actor_type();

        // 1. Create ONE release_intent for this release request
        let release_intent = sqlx::query!(
            "
            INSERT INTO release_intents (
                artifact,
                annotation_id,
                project_id,
                actor_id,
                actor_type
            ) VALUES (
                $1,
                $2,
                $3,
                $4,
                $5
            )
            RETURNING id
            ",
            artifact_id,
            annotation_id,
            project_id,
            actor_id,
            actor_type,
        )
        .fetch_one(&self.db)
        .await
        .context("create release_intent")?;

        let mut created_releases = Vec::new();

        // 2. Create a release_states row for each destination via the event store.
        // The partial unique index prevents duplicate in-flight releases.
        for dest in &destination_recs {
            use crate::services::release_event_store::CreateReleaseParams;

            event_store
                .create_release(CreateReleaseParams {
                    release_intent_id: release_intent.id,
                    project_id,
                    destination_id: dest.id,
                    artifact_id: *artifact_id,
                    actor: actor.clone(),
                    force,
                    stage_id: None,
                })
                .await
                .context(anyhow::anyhow!(
                    "create release: {} to {}",
                    artifact_id,
                    dest.id
                ))?;

            created_releases.push(CreatedRelease {
                destination: dest.name.clone(),
                environment: dest.environment.clone(),
            });
        }

        Ok(CreatedReleaseIntent {
            release_intent_id: release_intent.id,
            releases: created_releases,
            organisation,
            project,
        })
    }

    /// Pipeline-aware release: create intent with DAG stages, then only activate root stages.
    #[allow(clippy::too_many_arguments)]
    async fn release_with_pipeline(
        &self,
        artifact_id: &ArtifactID,
        annotation_id: &Uuid,
        project_id: &Uuid,
        organisation: &str,
        project: &str,
        actor: &Actor,
        event_store: &crate::services::release_event_store::ReleaseEventStore,
        force: bool,
        pipeline_rec: crate::services::release_pipeline::PipelineRecord,
    ) -> anyhow::Result<CreatedReleaseIntent> {
        use crate::services::release_pipeline::{
            PipelineStages, find_ready_stages, init_stage_states, StageState, StageStatus,
        };

        let stages: PipelineStages = serde_json::from_value(pipeline_rec.stages)
            .context("parse pipeline stages")?;

        let mut stage_states = init_stage_states(&stages);
        let stages_json = serde_json::to_value(&stages)?;
        let stage_states_json = serde_json::to_value(&stage_states)?;

        let actor_id = actor.actor_id();
        let actor_type = actor.actor_type();

        // Create release intent with pipeline data
        let release_intent = sqlx::query!(
            "INSERT INTO release_intents (
                artifact, annotation_id, project_id,
                actor_id, actor_type, stages, stage_states
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id",
            artifact_id,
            annotation_id,
            project_id,
            actor_id,
            actor_type,
            stages_json,
            stage_states_json,
        )
        .fetch_one(&self.db)
        .await
        .context("create pipeline release_intent")?;

        let mut created_releases = Vec::new();
        let now = chrono::Utc::now().to_rfc3339();

        // Activate root stages (those with no dependencies)
        let ready = find_ready_stages(&stages, &stage_states);

        for stage_id in &ready {
            let Some(stage_def) = stages.get(stage_id) else {
                continue;
            };

            match stage_def.stage_type.as_str() {
                "deploy" => {
                    let env = stage_def.environment.as_deref().unwrap_or("");

                    let dest_recs = sqlx::query!(
                        r#"SELECT d.id, d.name, d.environment
                         FROM destinations d
                         JOIN environments e ON d.environment_id = e.id
                         WHERE e.name = $1"#,
                        env,
                    )
                    .fetch_all(&self.db)
                    .await
                    .context("resolve destinations for pipeline stage")?;

                    let mut release_ids = Vec::new();
                    for dest in &dest_recs {
                        use crate::services::release_event_store::CreateReleaseParams;

                        let rid = event_store
                            .create_release(CreateReleaseParams {
                                release_intent_id: release_intent.id,
                                project_id: *project_id,
                                destination_id: dest.id,
                                artifact_id: *artifact_id,
                                actor: actor.clone(),
                                force,
                                stage_id: Some(stage_id.clone()),
                            })
                            .await
                            .context("create pipeline release")?;

                        release_ids.push(rid.to_string());
                        created_releases.push(CreatedRelease {
                            destination: dest.name.clone(),
                            environment: dest.environment.clone(),
                        });
                    }

                    stage_states.insert(
                        stage_id.clone(),
                        StageState {
                            status: StageStatus::Active,
                            release_ids: Some(release_ids),
                            started_at: Some(now.clone()),
                            ..StageState::pending()
                        },
                    );

                    tracing::info!(
                        release_intent_id = %release_intent.id,
                        stage_id,
                        dest_count = dest_recs.len(),
                        "pipeline: activated root deploy stage"
                    );
                }
                "wait" => {
                    let duration = stage_def.duration_seconds.unwrap_or(0);
                    let wait_until = chrono::Utc::now() + chrono::Duration::seconds(duration);

                    stage_states.insert(
                        stage_id.clone(),
                        StageState {
                            status: StageStatus::Active,
                            started_at: Some(now.clone()),
                            wait_until: Some(wait_until.to_rfc3339()),
                            ..StageState::pending()
                        },
                    );

                    tracing::info!(
                        release_intent_id = %release_intent.id,
                        stage_id,
                        duration,
                        "pipeline: activated root wait stage"
                    );
                }
                other => {
                    tracing::warn!(
                        stage_id,
                        stage_type = other,
                        "pipeline: unknown stage type, skipping"
                    );
                }
            }
        }

        // Persist updated stage_states after activating root stages
        let stage_states_json = serde_json::to_value(&stage_states)?;
        sqlx::query!(
            "UPDATE release_intents SET stage_states = $2 WHERE id = $1",
            release_intent.id,
            stage_states_json,
        )
        .execute(&self.db)
        .await
        .context("update stage_states after root activation")?;

        Ok(CreatedReleaseIntent {
            release_intent_id: release_intent.id,
            releases: created_releases,
            organisation: organisation.to_string(),
            project: project.to_string(),
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
                    rs.release_id as release_id,
                    rs.release_intent_id,
                    rs.destination_id,
                    rs.status,
                    d.name as destination
                FROM release_states rs
                JOIN destinations d ON rs.destination_id = d.id
                WHERE rs.release_intent_id = $1
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
                    p.organisation as organisation,
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
                organisation: rec.organisation,
                project: rec.project,
            },
            destinations: Vec::new(),
            created_at: rec.created,
        })
    }

    pub async fn get_release_annotation_by_project(
        &self,
        organisation: &str,
        project: &str,
    ) -> anyhow::Result<Vec<ReleaseAnnotation>> {
        // Get annotations without destinations — destination state is fetched
        // separately via get_destination_states and matched client-side by artifact_id.
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
                    p.organisation       as organisation,
                    p.project            as project
                FROM annotations a
                JOIN projects p ON a.project_id = p.id
                WHERE
                        p.organisation = $1
                    AND p.project = $2
                ORDER BY a.created DESC, a.id
            "#,
            organisation,
            project
        )
        .fetch_all(&self.db)
        .await
        .context("get annotations (db)")?;

        let mut annotations: Vec<ReleaseAnnotation> = Vec::new();

        for rec in recs {
            let annotation = ReleaseAnnotation {
                id: rec.id,
                artifact_id: rec.artifact_id,
                slug: rec.slug,
                metadata: serde_json::from_value(rec.metadata).context("metadata")?,
                source: serde_json::from_value(rec.source).context("source")?,
                context: serde_json::from_value(rec.context).context("context")?,
                project: Project {
                    organisation: rec.organisation,
                    project: rec.project,
                },
                destinations: Vec::new(),
                created_at: rec.created,
            };

            annotations.push(annotation);
        }

        Ok(annotations)
    }

    pub async fn get_organisations(&self) -> anyhow::Result<Vec<OrganisationName>> {
        // TODO: consider if we should cursor this
        let recs = sqlx::query!(
            "
                SELECT DISTINCT organisation FROM projects;
            "
        )
        .fetch_all(&self.db)
        .await
        .context("get organisations (db)")?;

        Ok(recs.into_iter().map(|r| r.organisation.into()).collect())
    }

    pub async fn get_projects_by_organisation(
        &self,
        organisation: &OrganisationName,
    ) -> anyhow::Result<Vec<ProjectName>> {
        // TODO: consider if we should cursor this
        let recs = sqlx::query!(
            "
                SELECT project
                FROM projects
                WHERE organisation = $1;
            ",
            organisation.as_str(),
        )
        .fetch_all(&self.db)
        .await
        .context("get projects (db)")?;

        Ok(recs.into_iter().map(|r| r.project.into()).collect())
    }

    pub async fn get_project_id(
        &self,
        organisation: &str,
        project: &str,
    ) -> anyhow::Result<Uuid> {
        let rec = sqlx::query!(
            "SELECT id FROM projects WHERE organisation = $1 AND project = $2",
            organisation,
            project
        )
        .fetch_one(&self.db)
        .await
        .context("project not found")?;

        Ok(rec.id)
    }

    pub async fn get_project_context(&self, project_id: &Uuid) -> anyhow::Result<(String, String)> {
        let rec = sqlx::query!(
            "SELECT organisation, project FROM projects WHERE id = $1",
            project_id
        )
        .fetch_one(&self.db)
        .await
        .context("get project context")?;

        Ok((rec.organisation, rec.project))
    }

    /// Get annotation details by artifact_id (for enriching notifications).
    pub async fn get_annotation_context(
        &self,
        artifact_id: &Uuid,
    ) -> anyhow::Result<AnnotationContext> {
        let rec = sqlx::query!(
            r#"
            SELECT slug, source, context, ref
            FROM annotations
            WHERE artifact_id = $1
            "#,
            artifact_id,
        )
        .fetch_one(&self.db)
        .await
        .context("get annotation context")?;

        let source: Source = serde_json::from_value(rec.source).unwrap_or(Source {
            username: None,
            email: None,
            source_type: None,
            run_url: None,
        });
        let context: ArtifactContext =
            serde_json::from_value(rec.context).unwrap_or(ArtifactContext {
                title: String::new(),
                description: None,
                web: None,
                pr: None,
            });
        let reference: Reference = serde_json::from_value(rec.r#ref).unwrap_or(Reference {
            commit_sha: String::new(),
            commit_branch: None,
            commit_message: None,
            version: None,
            repo_url: None,
        });

        Ok(AnnotationContext {
            slug: rec.slug,
            source,
            context,
            reference,
        })
    }

    pub async fn get_releases_by_actor(
        &self,
        actor_id: &Uuid,
        actor_type: &str,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<ReleaseIntentSummary>> {
        let fetch_limit = limit + 1;

        let recs = sqlx::query!(
            r#"
            SELECT
                ri.id as release_intent_id,
                ri.artifact as artifact_id,
                ri.created as created_at,
                p.organisation,
                p.project,
                d.name as "destination_name?",
                d.environment as "destination_env?",
                r.status as "status?"
            FROM release_intents ri
            JOIN projects p ON ri.project_id = p.id
            LEFT JOIN release_states r ON r.release_intent_id = ri.id
            LEFT JOIN destinations d ON r.destination_id = d.id
            WHERE ri.actor_id = $1 AND ri.actor_type = $2
            ORDER BY ri.created DESC
            LIMIT $3 OFFSET $4
            "#,
            actor_id,
            actor_type,
            fetch_limit,
            offset,
        )
        .fetch_all(&self.db)
        .await
        .context("get_releases_by_actor")?;

        // Group by release_intent_id
        let mut map: indexmap::IndexMap<Uuid, ReleaseIntentSummary> = indexmap::IndexMap::new();

        for rec in recs {
            let dest = match (rec.destination_name, rec.destination_env, rec.status) {
                (Some(name), Some(env), Some(status)) => Some(ReleaseDestinationStatus {
                    destination: name,
                    environment: env,
                    status,
                }),
                _ => None,
            };

            if let Some(entry) = map.get_mut(&rec.release_intent_id) {
                if let Some(d) = dest {
                    entry.destinations.push(d);
                }
            } else {
                let mut destinations = Vec::new();
                if let Some(d) = dest {
                    destinations.push(d);
                }
                map.insert(
                    rec.release_intent_id,
                    ReleaseIntentSummary {
                        release_intent_id: rec.release_intent_id,
                        artifact_id: rec.artifact_id,
                        project: Project {
                            organisation: rec.organisation,
                            project: rec.project,
                        },
                        destinations,
                        created_at: rec.created_at,
                    },
                );
            }
        }

        let mut results: Vec<ReleaseIntentSummary> = map.into_values().collect();
        results.truncate(limit as usize);
        Ok(results)
    }

    pub async fn get_destinations(&self, organisation: &str) -> anyhow::Result<Vec<Destination>> {
        let recs = sqlx::query!(
            "
                SELECT
                    id,
                    organisation,
                    name,
                    metadata,
                    environment,
                    type_organisation,
                    type_name,
                    type_version
                FROM destinations
                WHERE organisation = $1
            ",
            organisation,
        )
        .fetch_all(&self.db)
        .await
        .context("get destinations (db)")?;

        recs.into_iter()
            .map(|r| {
                Ok(Destination::new(
                    &r.organisation.to_string(),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_url: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct ArtifactContext {
    pub title: String,
    pub description: Option<String>,
    pub web: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Reference {
    pub commit_sha: String,
    pub commit_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
}

pub struct Project {
    pub organisation: String,
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
    pub status: String,
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
    pub organisation: String,
    pub project: String,
}

pub struct CreatedRelease {
    pub destination: String,
    pub environment: String,
}


pub struct ReleaseIntentSummary {
    pub release_intent_id: Uuid,
    pub artifact_id: Uuid,
    pub project: Project,
    pub destinations: Vec<ReleaseDestinationStatus>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct ReleaseDestinationStatus {
    pub destination: String,
    pub environment: String,
    pub status: String,
}

pub struct AnnotationContext {
    pub slug: String,
    pub source: Source,
    pub context: ArtifactContext,
    pub reference: Reference,
}
