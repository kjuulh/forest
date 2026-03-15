use anyhow::Context;
use forest_event_store::EventStore;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domains::trigger::{
    self, AnnotationMatchData, CreateTriggerParams, TriggerAggregate, TriggerMatch,
    TriggerPatterns, TriggerTargets, UpdateTriggerParams,
};

// ============================================================
// Projection record (read model — matches `triggers` table)
// ============================================================

pub struct TriggerRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub branch_pattern: Option<String>,
    pub title_pattern: Option<String>,
    pub author_pattern: Option<String>,
    pub commit_message_pattern: Option<String>,
    pub source_type_pattern: Option<String>,
    pub target_environments: Vec<String>,
    pub target_destinations: Vec<String>,
    pub force_release: bool,
    pub use_pipeline: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

// ============================================================
// Service — orchestrates aggregate + projections
// ============================================================

#[derive(Clone)]
pub struct TriggerAggregateService {
    event_store: EventStore,
    db: PgPool,
}

impl TriggerAggregateService {
    pub fn new(event_store: EventStore, db: PgPool) -> Self {
        Self { event_store, db }
    }

    // ----------------------------------------------------------
    // Commands
    // ----------------------------------------------------------

    pub async fn create(
        &self,
        project_id: Uuid,
        name: String,
        patterns: TriggerPatterns,
        targets: TriggerTargets,
        force_release: bool,
        use_pipeline: bool,
    ) -> anyhow::Result<TriggerRecord> {
        let key = trigger::stream_key(&project_id, &name);
        let mut root = self
            .event_store
            .load_or_default::<TriggerAggregate>(&key)
            .await?;

        let trigger_id = TriggerAggregate::create(
            &mut root,
            CreateTriggerParams {
                project_id,
                name: name.clone(),
                patterns: patterns.clone(),
                targets: targets.clone(),
                force_release,
                use_pipeline,
            },
        )?;

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO triggers (
                            id, project_id, name,
                            branch_pattern, title_pattern, author_pattern,
                            commit_message_pattern, source_type_pattern,
                            target_environments, target_destinations,
                            force_release, use_pipeline
                        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
                    )
                    .bind(trigger_id)
                    .bind(project_id)
                    .bind(&name)
                    .bind(&patterns.branch)
                    .bind(&patterns.title)
                    .bind(&patterns.author)
                    .bind(&patterns.commit_message)
                    .bind(&patterns.source_type)
                    .bind(&targets.environments)
                    .bind(&targets.destinations)
                    .bind(force_release)
                    .bind(use_pipeline)
                    .execute(&mut **tx)
                    .await
                    .context("insert trigger projection")?;
                    Ok(())
                })
            })
            .await?;

        // Read back the full record from the projection
        self.get_by_name(&project_id, &root.state.name)
            .await?
            .context("trigger projection not found after create")
    }

    pub async fn update(
        &self,
        project_id: &Uuid,
        name: &str,
        enabled: Option<bool>,
        patterns: Option<TriggerPatterns>,
        targets: Option<TriggerTargets>,
        force_release: Option<bool>,
        use_pipeline: Option<bool>,
    ) -> anyhow::Result<TriggerRecord> {
        let key = trigger::stream_key(project_id, name);
        let mut root = self
            .event_store
            .load_or_default::<TriggerAggregate>(&key)
            .await?;

        // Handle enabled toggle as a separate command
        if let Some(enabled) = enabled
            && enabled != root.state.enabled
        {
            TriggerAggregate::toggle_enabled(&mut root, enabled)?;
        }

        // Handle config updates
        let has_config_update = patterns.is_some()
            || targets.is_some()
            || force_release.is_some()
            || use_pipeline.is_some();

        if has_config_update {
            TriggerAggregate::update(
                &mut root,
                UpdateTriggerParams {
                    patterns: patterns.clone(),
                    targets: targets.clone(),
                    force_release,
                    use_pipeline,
                },
            )?;
        }

        if !root.has_pending() {
            // Nothing changed — just return the current record
            return self
                .get_by_name(project_id, name)
                .await?
                .context("trigger not found");
        }

        let name_owned = name.to_string();
        let project_id_owned = *project_id;
        let state = root.state.clone_for_projection();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    sqlx::query(
                        "UPDATE triggers SET
                            enabled = $3,
                            branch_pattern = $4,
                            title_pattern = $5,
                            author_pattern = $6,
                            commit_message_pattern = $7,
                            source_type_pattern = $8,
                            target_environments = $9,
                            target_destinations = $10,
                            force_release = $11,
                            use_pipeline = $12,
                            updated_at = now()
                        WHERE project_id = $1 AND name = $2",
                    )
                    .bind(project_id_owned)
                    .bind(&name_owned)
                    .bind(state.enabled)
                    .bind(&state.patterns.branch)
                    .bind(&state.patterns.title)
                    .bind(&state.patterns.author)
                    .bind(&state.patterns.commit_message)
                    .bind(&state.patterns.source_type)
                    .bind(&state.targets.environments)
                    .bind(&state.targets.destinations)
                    .bind(state.force_release)
                    .bind(state.use_pipeline)
                    .execute(&mut **tx)
                    .await
                    .context("update trigger projection")?;
                    Ok(())
                })
            })
            .await?;

        self.get_by_name(project_id, name)
            .await?
            .context("trigger not found after update")
    }

    pub async fn delete(&self, project_id: &Uuid, name: &str) -> anyhow::Result<()> {
        let key = trigger::stream_key(project_id, name);
        let mut root = self
            .event_store
            .load_or_default::<TriggerAggregate>(&key)
            .await?;

        TriggerAggregate::delete(&mut root)?;

        let project_id_owned = *project_id;
        let name_owned = name.to_string();

        self.event_store
            .save_with(&mut root, move |_events, tx| {
                Box::pin(async move {
                    let res = sqlx::query(
                        "DELETE FROM triggers WHERE project_id = $1 AND name = $2",
                    )
                    .bind(project_id_owned)
                    .bind(&name_owned)
                    .execute(&mut **tx)
                    .await
                    .context("delete trigger projection")?;

                    if res.rows_affected() != 1 {
                        anyhow::bail!("trigger projection not found for delete");
                    }
                    Ok(())
                })
            })
            .await?;

        Ok(())
    }

    // ----------------------------------------------------------
    // Queries (read from projections)
    // ----------------------------------------------------------

    pub async fn list(&self, project_id: &Uuid) -> anyhow::Result<Vec<TriggerRecord>> {
        let recs = sqlx::query_as!(
            TriggerRecord,
            r#"SELECT
                id, project_id, name, enabled,
                branch_pattern, title_pattern, author_pattern,
                commit_message_pattern, source_type_pattern,
                target_environments, target_destinations,
                force_release, use_pipeline, created_at, updated_at
            FROM triggers
            WHERE project_id = $1
            ORDER BY name"#,
            project_id,
        )
        .fetch_all(&self.db)
        .await
        .context("list triggers")?;

        Ok(recs)
    }

    pub async fn evaluate(
        &self,
        project_id: &Uuid,
        data: &AnnotationMatchData,
    ) -> anyhow::Result<Vec<TriggerMatch>> {
        let triggers = sqlx::query_as!(
            TriggerRecord,
            r#"SELECT
                id, project_id, name, enabled,
                branch_pattern, title_pattern, author_pattern,
                commit_message_pattern, source_type_pattern,
                target_environments, target_destinations,
                force_release, use_pipeline, created_at, updated_at
            FROM triggers
            WHERE project_id = $1 AND enabled = true
            ORDER BY name"#,
            project_id,
        )
        .fetch_all(&self.db)
        .await
        .context("evaluate triggers")?;

        let mut matches = Vec::new();

        for t in triggers {
            let patterns = TriggerPatterns {
                branch: t.branch_pattern,
                title: t.title_pattern,
                author: t.author_pattern,
                commit_message: t.commit_message_pattern,
                source_type: t.source_type_pattern,
            };

            if trigger::matches_trigger(&patterns, data) {
                matches.push(TriggerMatch {
                    trigger_name: t.name,
                    target_environments: t.target_environments,
                    target_destinations: t.target_destinations,
                    force_release: t.force_release,
                    use_pipeline: t.use_pipeline,
                });
            }
        }

        Ok(matches)
    }

    async fn get_by_name(
        &self,
        project_id: &Uuid,
        name: &str,
    ) -> anyhow::Result<Option<TriggerRecord>> {
        let rec = sqlx::query_as!(
            TriggerRecord,
            r#"SELECT
                id, project_id, name, enabled,
                branch_pattern, title_pattern, author_pattern,
                commit_message_pattern, source_type_pattern,
                target_environments, target_destinations,
                force_release, use_pipeline, created_at, updated_at
            FROM triggers
            WHERE project_id = $1 AND name = $2"#,
            project_id,
            name,
        )
        .fetch_optional(&self.db)
        .await
        .context("get trigger by name")?;

        Ok(rec)
    }
}

// ============================================================
// Helper for projection writes
// ============================================================

impl TriggerAggregate {
    /// Clone the fields needed for writing projection updates inside `save_with()`.
    fn clone_for_projection(&self) -> TriggerProjectionState {
        TriggerProjectionState {
            enabled: self.enabled,
            patterns: self.patterns.clone(),
            targets: self.targets.clone(),
            force_release: self.force_release,
            use_pipeline: self.use_pipeline,
        }
    }
}

struct TriggerProjectionState {
    enabled: bool,
    patterns: TriggerPatterns,
    targets: TriggerTargets,
    force_release: bool,
    use_pipeline: bool,
}

// ============================================================
// State integration
// ============================================================

pub trait TriggerAggregateServiceState {
    fn trigger_aggregate_service(&self) -> TriggerAggregateService;
}

impl TriggerAggregateServiceState for crate::state::State {
    fn trigger_aggregate_service(&self) -> TriggerAggregateService {
        TriggerAggregateService::new(self.event_store.clone(), self.db.clone())
    }
}
