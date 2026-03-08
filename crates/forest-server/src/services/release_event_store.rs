use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{actor::Actor, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseEventType {
    Requested,
    Assigned,
    Started,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

impl ReleaseEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Requested => "release.requested",
            Self::Assigned => "release.assigned",
            Self::Started => "release.started",
            Self::Succeeded => "release.succeeded",
            Self::Failed => "release.failed",
            Self::Cancelled => "release.cancelled",
            Self::TimedOut => "release.timed_out",
        }
    }

    fn target_status(&self) -> &'static str {
        match self {
            Self::Requested => "QUEUED",
            Self::Assigned => "ASSIGNED",
            Self::Started => "RUNNING",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
            Self::TimedOut => "TIMED_OUT",
        }
    }

    fn valid_from_statuses(&self) -> &'static [&'static str] {
        match self {
            Self::Requested => &[],
            Self::Assigned => &["QUEUED"],
            Self::Started => &["ASSIGNED"],
            Self::Succeeded => &["RUNNING"],
            Self::Failed => &["QUEUED", "ASSIGNED", "RUNNING"],
            Self::Cancelled => &["QUEUED", "ASSIGNED", "RUNNING"],
            Self::TimedOut => &["ASSIGNED", "RUNNING"],
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct EventPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub struct CreateReleaseParams {
    pub release_intent_id: Uuid,
    pub project_id: Uuid,
    pub destination_id: Uuid,
    pub artifact_id: Uuid,
    pub actor: Actor,
    pub force: bool,
    /// If part of a pipeline, the stage node ID this release belongs to.
    pub stage_id: Option<String>,
}

#[derive(Clone)]
pub struct ReleaseEventStore {
    db: PgPool,
    nats: async_nats::Client,
}

impl ReleaseEventStore {
    /// Create a new release: insert release_states + first event in one tx.
    /// Allows queuing behind in-flight releases. When `force` is set, cancels
    /// all other QUEUED releases for this project+destination first.
    pub async fn create_release(&self, params: CreateReleaseParams) -> anyhow::Result<Uuid> {
        let release_id = Uuid::now_v7();
        let mut tx = self.db.begin().await?;

        let actor_id = params.actor.actor_id();
        let actor_type = params.actor.actor_type();

        // Force release: cancel all QUEUED releases for this destination
        let cancelled_intent_ids = if params.force {
            let cancelled = sqlx::query!(
                "UPDATE release_states
                 SET status = 'CANCELLED', error_message = 'superseded by force release',
                     completed_at = now(), updated_at = now()
                 WHERE project_id = $1 AND destination_id = $2 AND status = 'QUEUED'
                 RETURNING release_id, release_intent_id",
                params.project_id,
                params.destination_id,
            )
            .fetch_all(&mut *tx)
            .await
            .context("cancel queued releases for force")?;

            let mut intent_ids = Vec::new();
            for row in &cancelled {
                sqlx::query!(
                    "INSERT INTO release_events (
                        release_id, event_type, payload, actor_id, actor_type
                    ) VALUES ($1, 'release.cancelled', $2, $3, $4)",
                    row.release_id,
                    serde_json::json!({"reason": "superseded by force release"}),
                    actor_id,
                    actor_type,
                )
                .execute(&mut *tx)
                .await?;
                intent_ids.push((row.release_id, row.release_intent_id));
            }

            if !cancelled.is_empty() {
                tracing::info!(
                    count = cancelled.len(),
                    project_id = %params.project_id,
                    destination_id = %params.destination_id,
                    "force release: cancelled queued releases"
                );
            }

            intent_ids
        } else {
            Vec::new()
        };

        sqlx::query!(
            "INSERT INTO release_states (
                release_id, release_intent_id, project_id,
                destination_id, artifact_id, status, stage_id
            ) VALUES ($1, $2, $3, $4, $5, 'QUEUED', $6)",
            release_id,
            params.release_intent_id,
            params.project_id,
            params.destination_id,
            params.artifact_id,
            params.stage_id,
        )
        .execute(&mut *tx)
        .await
        .context("insert release_states")?;

        sqlx::query!(
            "INSERT INTO release_events (
                release_id, event_type, payload, actor_id, actor_type
            ) VALUES ($1, 'release.requested', '{}', $2, $3)",
            release_id,
            actor_id,
            actor_type,
        )
        .execute(&mut *tx)
        .await
        .context("insert release.requested event")?;

        tx.commit().await?;

        // Publish NATS status updates for cancelled releases
        for (cancelled_id, cancelled_intent_id) in &cancelled_intent_ids {
            let subject = format!("forest.release.status.{}", cancelled_intent_id);
            let payload = serde_json::json!({
                "release_id": cancelled_id.to_string(),
                "status": "CANCELLED",
            });
            let _ = self.nats.publish(subject, payload.to_string().into()).await;
        }

        // Publish to NATS after commit (best-effort, fallback sweep catches misses)
        if let Err(e) = self
            .nats
            .publish("forest.release.queued", release_id.to_string().into())
            .await
        {
            tracing::warn!("failed to publish release.queued to NATS: {e}");
        }

        Ok(release_id)
    }

    /// Emit an event and update materialized state atomically.
    /// Returns Err if the current status doesn't allow this transition.
    pub async fn emit_event(
        &self,
        release_id: Uuid,
        event_type: ReleaseEventType,
        payload: EventPayload,
        actor: Option<&Actor>,
    ) -> anyhow::Result<()> {
        let target_status = event_type.target_status();
        let valid_from = event_type.valid_from_statuses();
        let payload_json = serde_json::to_value(&payload)?;

        let mut tx = self.db.begin().await?;

        // Lock the row and verify current status
        let row = sqlx::query!(
            "SELECT status, release_intent_id, project_id, destination_id, stage_id
             FROM release_states
             WHERE release_id = $1
             FOR UPDATE",
            release_id
        )
        .fetch_optional(&mut *tx)
        .await?
        .context("release not found")?;

        if !valid_from.contains(&row.status.as_str()) {
            anyhow::bail!(
                "invalid transition: cannot go from {} to {} (event: {})",
                row.status,
                target_status,
                event_type.as_str()
            );
        }

        let release_intent_id = row.release_intent_id;
        let project_id = row.project_id;
        let destination_id = row.destination_id;
        let stage_id = row.stage_id.clone();

        // Insert event
        let actor_id = actor.map(|a| a.actor_id());
        let actor_type = actor.map(|a| a.actor_type());

        sqlx::query!(
            "INSERT INTO release_events (
                release_id, event_type, payload, actor_id, actor_type
            ) VALUES ($1, $2, $3, $4, $5)",
            release_id,
            event_type.as_str(),
            payload_json,
            actor_id,
            actor_type,
        )
        .execute(&mut *tx)
        .await?;

        // Update materialized state based on event type
        match event_type {
            ReleaseEventType::Assigned => {
                sqlx::query!(
                    "UPDATE release_states SET
                        status = $2, runner_id = $3, assigned_at = now(),
                        last_heartbeat_at = now(), updated_at = now()
                     WHERE release_id = $1",
                    release_id,
                    target_status,
                    payload.runner_id,
                )
                .execute(&mut *tx)
                .await?;
            }
            ReleaseEventType::Started => {
                sqlx::query!(
                    "UPDATE release_states SET
                        status = $2, started_at = now(),
                        last_heartbeat_at = now(), updated_at = now()
                     WHERE release_id = $1",
                    release_id,
                    target_status,
                )
                .execute(&mut *tx)
                .await?;
            }
            ReleaseEventType::Succeeded
            | ReleaseEventType::Failed
            | ReleaseEventType::Cancelled
            | ReleaseEventType::TimedOut => {
                sqlx::query!(
                    "UPDATE release_states SET
                        status = $2, error_message = $3,
                        completed_at = now(), updated_at = now()
                     WHERE release_id = $1",
                    release_id,
                    target_status,
                    payload.error_message,
                )
                .execute(&mut *tx)
                .await?;
            }
            ReleaseEventType::Requested => {
                unreachable!("Requested is handled by create_release")
            }
        }

        tx.commit().await?;

        // Publish status change to NATS (best-effort)
        let nats_subject = format!("forest.release.status.{}", release_intent_id);
        let nats_payload = serde_json::json!({
            "release_id": release_id.to_string(),
            "status": target_status,
        });
        if let Err(e) = self
            .nats
            .publish(nats_subject, nats_payload.to_string().into())
            .await
        {
            tracing::warn!("failed to publish release status to NATS: {e}");
        }

        // On terminal events, signal the next queued release for this destination
        if matches!(
            event_type,
            ReleaseEventType::Succeeded
                | ReleaseEventType::Failed
                | ReleaseEventType::Cancelled
                | ReleaseEventType::TimedOut
        ) {
            if let Ok(Some(next_id)) = self
                .next_queued_for_destination(&project_id, &destination_id)
                .await
            {
                tracing::info!(
                    %next_id,
                    %release_id,
                    "signaling next queued release for destination"
                );
                let _ = self
                    .nats
                    .publish("forest.release.queued", next_id.to_string().into())
                    .await;
            }

            // If this release belongs to a pipeline stage, try to advance the pipeline
            if let Some(ref stage_id) = stage_id {
                if let Err(e) = self
                    .advance_pipeline(&release_intent_id, stage_id, event_type)
                    .await
                {
                    tracing::warn!(
                        %release_intent_id,
                        stage_id,
                        "failed to advance pipeline: {e:#}"
                    );
                }
            }
        }

        Ok(())
    }

    /// Pick up queued releases for the fallback sweep.
    /// Only returns releases whose destination has no ASSIGNED/RUNNING release.
    pub async fn pick_queued_releases(&self, limit: i64) -> anyhow::Result<Vec<QueuedRelease>> {
        let rows = sqlx::query_as!(
            QueuedRelease,
            "SELECT
                rs.release_id, rs.release_intent_id, rs.project_id,
                rs.destination_id, rs.artifact_id
             FROM release_states rs
             WHERE rs.status = 'QUEUED'
               AND NOT EXISTS (
                   SELECT 1 FROM release_states active
                   WHERE active.project_id = rs.project_id
                     AND active.destination_id = rs.destination_id
                     AND active.status IN ('ASSIGNED', 'RUNNING')
               )
             ORDER BY rs.queued_at
             LIMIT $1
             FOR UPDATE SKIP LOCKED",
            limit,
        )
        .fetch_all(&self.db)
        .await?;

        Ok(rows)
    }

    /// Find the next QUEUED release for a destination (oldest first).
    pub async fn next_queued_for_destination(
        &self,
        project_id: &Uuid,
        destination_id: &Uuid,
    ) -> anyhow::Result<Option<Uuid>> {
        let row = sqlx::query_scalar!(
            "SELECT release_id FROM release_states
             WHERE project_id = $1 AND destination_id = $2 AND status = 'QUEUED'
             ORDER BY queued_at ASC
             LIMIT 1",
            project_id,
            destination_id,
        )
        .fetch_optional(&self.db)
        .await?;

        Ok(row)
    }

    /// Get current state for a release by ID.
    pub async fn get_release_state(&self, release_id: &Uuid) -> anyhow::Result<ReleaseState> {
        let row = sqlx::query_as!(
            ReleaseState,
            "SELECT
                release_id, release_intent_id, project_id,
                destination_id, artifact_id, status,
                runner_id, error_message,
                queued_at, assigned_at, started_at, completed_at, updated_at
             FROM release_states
             WHERE release_id = $1",
            release_id,
        )
        .fetch_one(&self.db)
        .await
        .context("release state not found")?;

        Ok(row)
    }

    /// Get all release states for a given intent.
    pub async fn get_states_by_intent(
        &self,
        release_intent_id: &Uuid,
    ) -> anyhow::Result<Vec<ReleaseStateWithDestination>> {
        let rows = sqlx::query_as!(
            ReleaseStateWithDestination,
            r#"SELECT
                rs.release_id,
                rs.release_intent_id,
                rs.destination_id,
                rs.status,
                d.name as destination_name,
                d.environment as destination_environment
             FROM release_states rs
             JOIN destinations d ON rs.destination_id = d.id
             WHERE rs.release_intent_id = $1"#,
            release_intent_id,
        )
        .fetch_all(&self.db)
        .await
        .context("get states by intent")?;

        Ok(rows)
    }

    /// Get release states by actor for history view.
    pub async fn get_states_by_actor(
        &self,
        actor_id: &Uuid,
        actor_type: &str,
        limit: i64,
        offset: i64,
    ) -> anyhow::Result<Vec<ReleaseIntentSummaryRow>> {
        let fetch_limit = limit + 1;

        let rows = sqlx::query_as!(
            ReleaseIntentSummaryRow,
            r#"SELECT
                ri.id as release_intent_id,
                ri.artifact as artifact_id,
                ri.created as created_at,
                p.organisation,
                p.project,
                d.name as "destination_name?",
                d.environment as "destination_env?",
                rs.status as "status?"
            FROM release_intents ri
            JOIN projects p ON ri.project_id = p.id
            LEFT JOIN release_states rs ON rs.release_intent_id = ri.id
            LEFT JOIN destinations d ON rs.destination_id = d.id
            WHERE ri.actor_id = $1 AND ri.actor_type = $2
            ORDER BY ri.created DESC
            LIMIT $3 OFFSET $4"#,
            actor_id,
            actor_type,
            fetch_limit,
            offset,
        )
        .fetch_all(&self.db)
        .await
        .context("get_states_by_actor")?;

        Ok(rows)
    }

    /// Get release state per destination: the latest completed release ("current")
    /// plus all in-flight releases with ascending queue position.
    pub async fn get_destination_states(
        &self,
        organisation: &str,
        project_id: Option<&Uuid>,
    ) -> anyhow::Result<Vec<DestinationReleaseRow>> {
        let rows = sqlx::query_as!(
            DestinationReleaseRow,
            r#"WITH in_flight AS (
                SELECT
                    rs.*,
                    ROW_NUMBER() OVER (PARTITION BY rs.destination_id ORDER BY rs.queued_at ASC)
                        as queue_pos
                FROM release_states rs
                JOIN destinations d ON d.id = rs.destination_id
                WHERE d.organisation = $1
                  AND ($2::uuid IS NULL OR rs.project_id = $2)
                  AND rs.status IN ('QUEUED', 'ASSIGNED', 'RUNNING')
            ),
            current_release AS (
                SELECT DISTINCT ON (rs.destination_id)
                    rs.*
                FROM release_states rs
                JOIN destinations d ON d.id = rs.destination_id
                WHERE d.organisation = $1
                  AND ($2::uuid IS NULL OR rs.project_id = $2)
                  AND rs.status IN ('SUCCEEDED', 'FAILED', 'CANCELLED', 'TIMED_OUT')
                ORDER BY rs.destination_id, rs.completed_at DESC NULLS LAST
            )
            SELECT
                d.id as destination_id,
                d.name as destination_name,
                d.environment,
                r.release_id as "release_id!",
                r.artifact_id as "artifact_id!",
                r.status as "status!",
                r.error_message,
                r.queued_at as "queued_at!",
                r.completed_at,
                r.queue_pos as queue_position
            FROM destinations d
            JOIN (
                SELECT release_id, destination_id, artifact_id, status,
                       error_message, queued_at, completed_at,
                       NULL::bigint as queue_pos
                FROM current_release
                UNION ALL
                SELECT release_id, destination_id, artifact_id, status,
                       error_message, queued_at, completed_at,
                       queue_pos
                FROM in_flight
            ) r ON r.destination_id = d.id
            WHERE d.organisation = $1
            ORDER BY d.environment, d.name, r.queue_pos NULLS FIRST, r.queued_at DESC"#,
            organisation,
            project_id,
        )
        .fetch_all(&self.db)
        .await
        .context("get destination states")?;

        Ok(rows)
    }

    /// Find stuck releases for the reaper.
    pub async fn find_stuck_releases(
        &self,
        assigned_timeout_secs: i64,
        running_timeout_secs: i64,
    ) -> anyhow::Result<Vec<StuckRelease>> {
        let rows = sqlx::query_as!(
            StuckRelease,
            r#"SELECT release_id, release_intent_id, status, runner_id
             FROM release_states
             WHERE
               (status = 'ASSIGNED' AND assigned_at < now() - make_interval(secs => $1::double precision))
               OR (status = 'RUNNING' AND started_at < now() - make_interval(secs => $2::double precision))
             FOR UPDATE SKIP LOCKED"#,
            assigned_timeout_secs as f64,
            running_timeout_secs as f64,
        )
        .fetch_all(&self.db)
        .await?;

        Ok(rows)
    }

    /// Update the heartbeat timestamp for an active release.
    pub async fn heartbeat_release(&self, release_id: &Uuid) -> anyhow::Result<()> {
        sqlx::query!(
            "UPDATE release_states SET last_heartbeat_at = now()
             WHERE release_id = $1 AND status IN ('ASSIGNED', 'RUNNING')",
            release_id,
        )
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Update heartbeat for all active releases owned by a runner.
    pub async fn heartbeat_runner_releases(&self, runner_id: &str) -> anyhow::Result<()> {
        sqlx::query!(
            "UPDATE release_states SET last_heartbeat_at = now()
             WHERE runner_id = $1 AND status IN ('ASSIGNED', 'RUNNING')",
            runner_id,
        )
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Advance a pipeline after a stage's release reaches a terminal state.
    /// Checks if all releases for the completed stage are done, updates stage_states,
    /// and activates the next ready stages in the DAG.
    pub async fn advance_pipeline(
        &self,
        release_intent_id: &Uuid,
        completed_stage_id: &str,
        _event_type: ReleaseEventType,
    ) -> anyhow::Result<()> {
        use crate::services::release_pipeline::{
            PipelineStages, StageState, StageStates, StageStatus, find_ready_stages,
            has_failed_dependency,
        };

        let mut tx = self.db.begin().await?;

        // Lock the release intent and load pipeline data
        let intent = sqlx::query!(
            "SELECT id, artifact, project_id, stages, stage_states
             FROM release_intents
             WHERE id = $1
             FOR UPDATE",
            release_intent_id,
        )
        .fetch_one(&mut *tx)
        .await
        .context("load release intent for pipeline advancement")?;

        let Some(stages_json) = intent.stages else {
            // No pipeline — nothing to advance
            return Ok(());
        };

        let stages: PipelineStages =
            serde_json::from_value(stages_json).context("parse pipeline stages")?;

        let mut stage_states: StageStates = intent
            .stage_states
            .map(|v| serde_json::from_value(v))
            .transpose()
            .context("parse stage_states")?
            .unwrap_or_default();

        // Check if ALL releases for the completed stage are terminal
        let stage_releases = sqlx::query!(
            "SELECT release_id, status FROM release_states
             WHERE release_intent_id = $1 AND stage_id = $2",
            release_intent_id,
            completed_stage_id,
        )
        .fetch_all(&mut *tx)
        .await?;

        let all_succeeded = stage_releases
            .iter()
            .all(|r| r.status == "SUCCEEDED");
        let any_failed = stage_releases
            .iter()
            .any(|r| matches!(r.status.as_str(), "FAILED" | "CANCELLED" | "TIMED_OUT"));
        let all_terminal = stage_releases
            .iter()
            .all(|r| matches!(r.status.as_str(), "SUCCEEDED" | "FAILED" | "CANCELLED" | "TIMED_OUT"));

        if !all_terminal {
            // Not all releases for this stage are done yet — wait
            tx.commit().await?;
            return Ok(());
        }

        let now = chrono::Utc::now().to_rfc3339();

        // Update the completed stage's state
        if all_succeeded {
            if let Some(state) = stage_states.get_mut(completed_stage_id) {
                state.status = StageStatus::Succeeded;
                state.completed_at = Some(now.clone());
            }
        } else if any_failed {
            if let Some(state) = stage_states.get_mut(completed_stage_id) {
                state.status = StageStatus::Failed;
                state.completed_at = Some(now.clone());
            }
        }

        // Cancel all PENDING stages whose dependencies have failed
        let stage_ids: Vec<String> = stages.keys().cloned().collect();
        for stage_id in &stage_ids {
            let is_pending = stage_states
                .get(stage_id)
                .map_or(true, |s| s.status == StageStatus::Pending);
            if is_pending && has_failed_dependency(stage_id, &stages, &stage_states) {
                stage_states.insert(
                    stage_id.clone(),
                    StageState {
                        status: StageStatus::Cancelled,
                        error_message: Some("upstream stage failed".into()),
                        completed_at: Some(now.clone()),
                        ..StageState::pending()
                    },
                );
            }
        }

        // Find next ready stages and activate them
        let ready = find_ready_stages(&stages, &stage_states);
        let mut new_release_ids: Vec<Uuid> = Vec::new();

        for stage_id in &ready {
            let Some(stage_def) = stages.get(stage_id) else {
                continue;
            };

            match stage_def.stage_type.as_str() {
                "deploy" => {
                    let env = stage_def.environment.as_deref().unwrap_or("");

                    // Resolve environment -> destinations
                    let dest_recs = sqlx::query!(
                        r#"SELECT d.id
                         FROM destinations d
                         JOIN environments e ON d.environment_id = e.id
                         WHERE e.name = $1"#,
                        env,
                    )
                    .fetch_all(&mut *tx)
                    .await
                    .context("resolve destinations for stage")?;

                    let mut release_ids = Vec::new();
                    for dest in &dest_recs {
                        let rid = Uuid::now_v7();
                        sqlx::query!(
                            "INSERT INTO release_states (
                                release_id, release_intent_id, project_id,
                                destination_id, artifact_id, status, stage_id
                            ) VALUES ($1, $2, $3, $4, $5, 'QUEUED', $6)",
                            rid,
                            release_intent_id,
                            intent.project_id,
                            dest.id,
                            intent.artifact,
                            stage_id.as_str(),
                        )
                        .execute(&mut *tx)
                        .await?;

                        sqlx::query!(
                            "INSERT INTO release_events (
                                release_id, event_type, payload
                            ) VALUES ($1, 'release.requested', '{}')",
                            rid,
                        )
                        .execute(&mut *tx)
                        .await?;

                        release_ids.push(rid.to_string());
                        new_release_ids.push(rid);
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
                        %release_intent_id,
                        stage_id,
                        dest_count = dest_recs.len(),
                        "pipeline: activated deploy stage"
                    );
                }
                "wait" => {
                    let duration = stage_def.duration_seconds.unwrap_or(0);
                    let wait_until = chrono::Utc::now()
                        + chrono::Duration::seconds(duration);

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
                        %release_intent_id,
                        stage_id,
                        duration,
                        "pipeline: activated wait stage (until {})",
                        wait_until
                    );
                }
                other => {
                    tracing::warn!(
                        %release_intent_id,
                        stage_id,
                        stage_type = other,
                        "pipeline: unknown stage type, skipping"
                    );
                }
            }
        }

        // Persist updated stage_states
        let stage_states_json = serde_json::to_value(&stage_states)?;
        sqlx::query!(
            "UPDATE release_intents SET stage_states = $2 WHERE id = $1",
            release_intent_id,
            stage_states_json,
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        // Signal newly queued releases via NATS (after commit)
        for rid in new_release_ids {
            let _ = self
                .nats
                .publish("forest.release.queued", rid.to_string().into())
                .await;
        }

        // Publish stage status updates
        let nats_subject = format!("forest.release.status.{}", release_intent_id);
        let nats_payload = serde_json::json!({
            "stage_id": completed_stage_id,
            "stage_status": if all_succeeded { "SUCCEEDED" } else { "FAILED" },
        });
        let _ = self
            .nats
            .publish(nats_subject, nats_payload.to_string().into())
            .await;

        Ok(())
    }

    /// Find wait stages that have expired. Returns (release_intent_id, stage_id) pairs.
    pub async fn find_expired_wait_stages(&self) -> anyhow::Result<Vec<ExpiredWaitStage>> {
        // Query intents that have active wait stages.
        // We check the JSONB for stages with status=ACTIVE and wait_until in the past.
        let rows = sqlx::query_as!(
            ExpiredWaitStageRow,
            r#"SELECT id as release_intent_id, stages, stage_states
             FROM release_intents
             WHERE stage_states IS NOT NULL
               AND stages IS NOT NULL
             FOR UPDATE SKIP LOCKED"#,
        )
        .fetch_all(&self.db)
        .await?;

        use crate::services::release_pipeline::{StageStates, StageStatus};

        let mut expired = Vec::new();
        let now = chrono::Utc::now();

        for row in rows {
            let Some(stage_states_json) = row.stage_states else {
                continue;
            };
            let stage_states: StageStates = match serde_json::from_value(stage_states_json) {
                Ok(s) => s,
                Err(_) => continue,
            };

            for (stage_id, state) in &stage_states {
                if state.status != StageStatus::Active {
                    continue;
                }
                if let Some(ref wait_until_str) = state.wait_until {
                    if let Ok(wait_until) = chrono::DateTime::parse_from_rfc3339(wait_until_str) {
                        if wait_until <= now {
                            expired.push(ExpiredWaitStage {
                                release_intent_id: row.release_intent_id,
                                stage_id: stage_id.clone(),
                            });
                        }
                    }
                }
            }
        }

        Ok(expired)
    }

    /// Complete a wait stage that has expired, marking it SUCCEEDED
    /// and advancing the pipeline.
    pub async fn complete_wait_stage(
        &self,
        release_intent_id: &Uuid,
        stage_id: &str,
    ) -> anyhow::Result<()> {
        use crate::services::release_pipeline::{StageStates, StageStatus};

        let mut tx = self.db.begin().await?;

        let intent = sqlx::query!(
            "SELECT stage_states FROM release_intents WHERE id = $1 FOR UPDATE",
            release_intent_id,
        )
        .fetch_one(&mut *tx)
        .await?;

        let Some(stage_states_json) = intent.stage_states else {
            return Ok(());
        };

        let mut stage_states: StageStates =
            serde_json::from_value(stage_states_json).context("parse stage_states")?;

        let Some(state) = stage_states.get_mut(stage_id) else {
            return Ok(());
        };

        if state.status != StageStatus::Active {
            return Ok(());
        }

        state.status = StageStatus::Succeeded;
        state.completed_at = Some(chrono::Utc::now().to_rfc3339());

        let stage_states_json = serde_json::to_value(&stage_states)?;
        sqlx::query!(
            "UPDATE release_intents SET stage_states = $2 WHERE id = $1",
            release_intent_id,
            stage_states_json,
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        tracing::info!(
            %release_intent_id,
            stage_id,
            "pipeline: wait stage completed"
        );

        // Use advance_pipeline with a dummy Succeeded event to trigger next stages
        self.advance_pipeline(release_intent_id, stage_id, ReleaseEventType::Succeeded)
            .await?;

        Ok(())
    }

    /// Find releases with stale heartbeats.
    pub async fn find_stale_heartbeats(
        &self,
        threshold_secs: i64,
    ) -> anyhow::Result<Vec<StuckRelease>> {
        let rows = sqlx::query_as!(
            StuckRelease,
            r#"SELECT release_id, release_intent_id, status, runner_id
             FROM release_states
             WHERE status IN ('ASSIGNED', 'RUNNING')
               AND last_heartbeat_at IS NOT NULL
               AND last_heartbeat_at < now() - make_interval(secs => $1::double precision)
             FOR UPDATE SKIP LOCKED"#,
            threshold_secs as f64,
        )
        .fetch_all(&self.db)
        .await?;

        Ok(rows)
    }
}

pub struct QueuedRelease {
    pub release_id: Uuid,
    pub release_intent_id: Uuid,
    pub project_id: Uuid,
    pub destination_id: Uuid,
    pub artifact_id: Uuid,
}

pub struct ReleaseState {
    pub release_id: Uuid,
    pub release_intent_id: Uuid,
    pub project_id: Uuid,
    pub destination_id: Uuid,
    pub artifact_id: Uuid,
    pub status: String,
    pub runner_id: Option<String>,
    pub error_message: Option<String>,
    pub queued_at: chrono::DateTime<chrono::Utc>,
    pub assigned_at: Option<chrono::DateTime<chrono::Utc>>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct ReleaseStateWithDestination {
    pub release_id: Uuid,
    pub release_intent_id: Uuid,
    pub destination_id: Uuid,
    pub status: String,
    pub destination_name: String,
    pub destination_environment: String,
}

pub struct ReleaseIntentSummaryRow {
    pub release_intent_id: Uuid,
    pub artifact_id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub organisation: String,
    pub project: String,
    pub destination_name: Option<String>,
    pub destination_env: Option<String>,
    pub status: Option<String>,
}

/// A single release row in the destination state view.
/// `kind` is either "current" (latest completed) or "queued" (in-flight).
pub struct DestinationReleaseRow {
    pub destination_id: Uuid,
    pub destination_name: String,
    pub environment: String,
    pub release_id: Uuid,
    pub artifact_id: Uuid,
    pub status: String,
    pub error_message: Option<String>,
    pub queued_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub queue_position: Option<i64>,
}

pub struct StuckRelease {
    pub release_id: Uuid,
    pub release_intent_id: Uuid,
    pub status: String,
    pub runner_id: Option<String>,
}

struct ExpiredWaitStageRow {
    pub release_intent_id: Uuid,
    pub stages: Option<serde_json::Value>,
    pub stage_states: Option<serde_json::Value>,
}

pub struct ExpiredWaitStage {
    pub release_intent_id: Uuid,
    pub stage_id: String,
}

pub trait ReleaseEventStoreState {
    fn release_event_store(&self) -> ReleaseEventStore;
}

impl ReleaseEventStoreState for State {
    fn release_event_store(&self) -> ReleaseEventStore {
        ReleaseEventStore {
            db: self.db.clone(),
            nats: self.nats.clone(),
        }
    }
}
