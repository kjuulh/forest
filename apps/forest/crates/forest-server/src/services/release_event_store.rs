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
    pub(crate) db: PgPool,
    pub(crate) nats: async_nats::Client,
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

        // Write org event into outbox (same transaction)
        let org_project = sqlx::query!(
            "SELECT organisation, project FROM projects WHERE id = $1",
            project_id,
        )
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(ref op) = org_project {
            let event_metadata = serde_json::json!({
                "status": target_status,
                "release_id": release_id.to_string(),
                "intent_id": release_intent_id.to_string(),
            });
            sqlx::query!(
                r#"INSERT INTO org_events (organisation, project, resource_type, action, resource_id, metadata)
                   VALUES ($1, $2, 'release', 'status_changed', $3, $4)"#,
                op.organisation,
                op.project,
                release_id.to_string(),
                event_metadata,
            )
            .execute(&mut *tx)
            .await?;
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

        // Nudge org event listeners
        if let Some(ref op) = org_project {
            let org_subject = format!("forest.events.{}", op.organisation);
            let _ = self.nats.publish(org_subject, "".into()).await;
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

            // If this release belongs to a pipeline, signal the coordinator to re-evaluate
            if stage_id.is_some() {
                let _ = self
                    .nats
                    .publish(
                        "forest.intent.evaluate",
                        release_intent_id.to_string().into(),
                    )
                    .await;
            } else {
                // Direct intent (no pipeline) — finalize if all child releases are terminal
                self.try_finalize_direct_intent(&release_intent_id).await;
            }
        }

        Ok(())
    }

    /// Finalize a direct (non-pipeline) intent when all child releases are terminal.
    async fn try_finalize_direct_intent(&self, intent_id: &Uuid) {
        let result: Result<_, anyhow::Error> = async {
            // Check if this is a direct intent (no stages) that's still ACTIVE
            let intent = sqlx::query!(
                "SELECT id, stages, status FROM release_intents WHERE id = $1",
                intent_id,
            )
            .fetch_optional(&self.db)
            .await?;

            let Some(intent) = intent else { return Ok(()) };
            if intent.stages.is_some() || intent.status != "ACTIVE" {
                return Ok(());
            }

            // Check if all child releases are terminal
            let non_terminal = sqlx::query_scalar!(
                r#"SELECT count(*) as "count!" FROM release_states
                 WHERE release_intent_id = $1
                   AND status NOT IN ('SUCCEEDED', 'FAILED', 'CANCELLED', 'TIMED_OUT')"#,
                intent_id,
            )
            .fetch_one(&self.db)
            .await?;

            if non_terminal > 0 {
                return Ok(());
            }

            // All terminal — determine intent status
            let any_failed = sqlx::query_scalar!(
                r#"SELECT count(*) as "count!" FROM release_states
                 WHERE release_intent_id = $1
                   AND status IN ('FAILED', 'TIMED_OUT')"#,
                intent_id,
            )
            .fetch_one(&self.db)
            .await?;

            let intent_status = if any_failed > 0 { "FAILED" } else { "SUCCEEDED" };

            sqlx::query!(
                "UPDATE release_intents SET status = $1, updated = now() WHERE id = $2",
                intent_status,
                intent_id,
            )
            .execute(&self.db)
            .await?;

            tracing::debug!(
                %intent_id,
                status = intent_status,
                "finalized direct intent"
            );

            Ok(())
        }
        .await;

        if let Err(e) = result {
            tracing::warn!(%intent_id, "failed to finalize direct intent: {e:#}");
        }
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
                queued_at, assigned_at, started_at, completed_at, updated_at,
                mode, plan_output
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
                r.release_intent_id as "release_intent_id!",
                r.artifact_id as "artifact_id!",
                r.status as "status!",
                r.error_message,
                r.queued_at as "queued_at!",
                r.assigned_at,
                r.started_at,
                r.completed_at,
                r.queue_pos as queue_position,
                r.stage_id
            FROM destinations d
            JOIN (
                SELECT release_id, release_intent_id, destination_id, artifact_id, status,
                       error_message, queued_at, assigned_at, started_at, completed_at,
                       NULL::bigint as queue_pos, stage_id
                FROM current_release
                UNION ALL
                SELECT release_id, release_intent_id, destination_id, artifact_id, status,
                       error_message, queued_at, assigned_at, started_at, completed_at,
                       queue_pos, stage_id
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

    /// Get active pipeline runs (release_intents that have a stages DAG and
    /// at least one non-terminal stage) for a given organisation/project.
    ///
    /// A pipeline is considered active if either:
    /// - It has in-flight release_states (deploy stages: QUEUED/ASSIGNED/RUNNING), OR
    /// - Its stage_states JSONB contains any PENDING or ACTIVE stage (covers wait stages
    ///   and deploy stages that haven't created release_states yet).
    pub async fn get_active_pipeline_runs(
        &self,
        organisation: &str,
        project_id: Option<&Uuid>,
    ) -> anyhow::Result<Vec<PipelineRunRow>> {
        let rows = sqlx::query_as!(
            PipelineRunRow,
            r#"SELECT
                ri.id as release_intent_id,
                ri.artifact as artifact_id,
                ri.created as created_at,
                ri.stages,
                ri.stage_states
            FROM release_intents ri
            JOIN projects p ON ri.project_id = p.id
            WHERE p.organisation = $1
              AND ($2::uuid IS NULL OR ri.project_id = $2)
              AND ri.stages IS NOT NULL
              AND (
                  -- Deploy stages: check release_states rows
                  EXISTS (
                      SELECT 1 FROM release_states rs
                      WHERE rs.release_intent_id = ri.id
                        AND rs.status IN ('QUEUED', 'ASSIGNED', 'RUNNING')
                  )
                  OR
                  -- Wait stages (and any non-terminal stage in the JSONB):
                  -- check if any stage value has status PENDING or ACTIVE
                  EXISTS (
                      SELECT 1
                      FROM jsonb_each(ri.stage_states) AS s(key, val)
                      WHERE val->>'status' IN ('PENDING', 'ACTIVE')
                  )
              )
            ORDER BY ri.created DESC"#,
            organisation,
            project_id,
        )
        .fetch_all(&self.db)
        .await
        .context("get active pipeline runs")?;

        Ok(rows)
    }

    /// Get release intent states: a release-centric view that returns
    /// intent metadata, pipeline stages, and all release steps.
    pub async fn get_release_intent_states(
        &self,
        organisation: &str,
        project_id: Option<&Uuid>,
        include_completed: bool,
    ) -> anyhow::Result<Vec<(ReleaseIntentRow, Vec<ReleaseStepRow>)>> {
        // Fetch release intents
        let intents = sqlx::query_as!(
            ReleaseIntentRow,
            r#"SELECT
                ri.id as release_intent_id,
                ri.artifact as artifact_id,
                p.project as project,
                ri.created as created_at,
                ri.stages,
                ri.stage_states
            FROM release_intents ri
            JOIN projects p ON ri.project_id = p.id
            WHERE p.organisation = $1
              AND ($2::uuid IS NULL OR ri.project_id = $2)
              AND (
                  $3::bool
                  OR EXISTS (
                      SELECT 1 FROM release_states rs
                      WHERE rs.release_intent_id = ri.id
                        AND rs.status IN ('QUEUED', 'ASSIGNED', 'RUNNING')
                  )
                  OR (
                      ri.stage_states IS NOT NULL
                      AND EXISTS (
                          SELECT 1
                          FROM jsonb_each(ri.stage_states) AS s(key, val)
                          WHERE val->>'status' IN ('PENDING', 'ACTIVE')
                      )
                  )
              )
            ORDER BY ri.created DESC
            LIMIT 50"#,
            organisation,
            project_id,
            include_completed,
        )
        .fetch_all(&self.db)
        .await
        .context("get release intent states")?;

        if intents.is_empty() {
            return Ok(Vec::new());
        }

        let intent_ids: Vec<Uuid> = intents.iter().map(|i| i.release_intent_id).collect();

        // Fetch all release steps for these intents in one query
        let steps = sqlx::query_as!(
            ReleaseStepRow,
            r#"SELECT
                rs.release_id,
                rs.release_intent_id,
                rs.stage_id,
                d.name as destination_name,
                d.environment,
                rs.status,
                rs.queued_at,
                rs.assigned_at,
                rs.started_at,
                rs.completed_at,
                rs.error_message
            FROM release_states rs
            JOIN destinations d ON d.id = rs.destination_id
            WHERE rs.release_intent_id = ANY($1)
            ORDER BY rs.queued_at ASC"#,
            &intent_ids,
        )
        .fetch_all(&self.db)
        .await
        .context("get release steps for intents")?;

        // Group steps by intent
        let mut steps_by_intent: std::collections::HashMap<Uuid, Vec<ReleaseStepRow>> =
            std::collections::HashMap::new();
        for step in steps {
            steps_by_intent
                .entry(step.release_intent_id)
                .or_default()
                .push(step);
        }

        let results = intents
            .into_iter()
            .map(|intent| {
                let intent_steps = steps_by_intent
                    .remove(&intent.release_intent_id)
                    .unwrap_or_default();
                (intent, intent_steps)
            })
            .collect();

        Ok(results)
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
    pub mode: String,
    pub plan_output: Option<String>,
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
pub struct DestinationReleaseRow {
    pub destination_id: Uuid,
    pub destination_name: String,
    pub environment: String,
    pub release_id: Uuid,
    pub artifact_id: Uuid,
    pub status: String,
    pub error_message: Option<String>,
    pub queued_at: chrono::DateTime<chrono::Utc>,
    pub assigned_at: Option<chrono::DateTime<chrono::Utc>>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub queue_position: Option<i64>,
    pub release_intent_id: Uuid,
    pub stage_id: Option<String>,
}

/// An active pipeline run with its stage definitions and runtime states.
pub struct PipelineRunRow {
    pub release_intent_id: Uuid,
    pub artifact_id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub stages: Option<serde_json::Value>,
    pub stage_states: Option<serde_json::Value>,
}

pub struct StuckRelease {
    pub release_id: Uuid,
    pub release_intent_id: Uuid,
    pub status: String,
    pub runner_id: Option<String>,
}



// ── Release intent states (release-centric view) ─────────────────────

pub struct ReleaseIntentRow {
    pub release_intent_id: Uuid,
    pub artifact_id: Uuid,
    pub project: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub stages: Option<serde_json::Value>,
    pub stage_states: Option<serde_json::Value>,
}

pub struct ReleaseStepRow {
    pub release_id: Uuid,
    pub release_intent_id: Uuid,
    pub stage_id: Option<String>,
    pub destination_name: String,
    pub environment: String,
    pub status: String,
    pub queued_at: chrono::DateTime<chrono::Utc>,
    pub assigned_at: Option<chrono::DateTime<chrono::Utc>>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error_message: Option<String>,
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

/// Check approval policies for a target environment within a transaction.
/// Returns `Some(reason)` if blocked, `None` if all policies pass.
pub(crate) async fn check_approval_policies(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: &Uuid,
    release_intent_id: Uuid,
    target_environment: &str,
) -> anyhow::Result<Option<String>> {
    let policies = sqlx::query!(
        r#"SELECT name, config FROM policies WHERE project_id = $1 AND enabled = true AND policy_type = 'approval'"#,
        project_id,
    )
    .fetch_all(&mut **tx)
    .await
    .context("load approval policies")?;

    for policy in policies {
        let config: serde_json::Value = policy.config;
        let target = config.get("target_environment").and_then(|v| v.as_str()).unwrap_or("");
        if target != target_environment { continue; }
        let required = config.get("required_approvals").and_then(|v| v.as_i64()).unwrap_or(1);

        let approved_count = sqlx::query_scalar!(
            r#"SELECT COUNT(*) as "count!" FROM approval_decisions
             WHERE release_intent_id = $1 AND target_environment = $2 AND decision = 'approved'"#,
            release_intent_id, target_environment,
        )
        .fetch_one(&mut **tx)
        .await
        .context("count approvals in pipeline")?;

        if approved_count < required {
            return Ok(Some(format!(
                "policy '{}': awaiting approval ({}/{} approvals)",
                policy.name, approved_count, required
            )));
        }
    }
    Ok(None)
}

/// Check soak_time policies for a target environment within a transaction.
/// Returns `Some(reason)` if blocked, `None` if all policies pass.
pub(crate) async fn check_soak_time_policies(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: &Uuid,
    artifact_id: &Uuid,
    target_environment: &str,
) -> anyhow::Result<Option<String>> {
    // Load enabled soak_time policies for this project
    let policies = sqlx::query!(
        r#"SELECT name, config
        FROM policies
        WHERE project_id = $1
          AND enabled = true
          AND policy_type = 'soak_time'"#,
        project_id,
    )
    .fetch_all(&mut **tx)
    .await
    .context("load soak_time policies")?;

    for policy in policies {
        let config: serde_json::Value = policy.config;
        let target = config
            .get("target_environment")
            .and_then(|v: &serde_json::Value| v.as_str())
            .unwrap_or("");

        if target != target_environment {
            continue;
        }

        let source_env = config
            .get("source_environment")
            .and_then(|v: &serde_json::Value| v.as_str())
            .unwrap_or("");
        let duration_secs = config
            .get("duration_seconds")
            .and_then(|v: &serde_json::Value| v.as_i64())
            .unwrap_or(0);

        // Find the most recent successful release of THIS artifact to the source environment.
        // Scoping by artifact prevents unrelated dev deploys from resetting the soak clock.
        let last_success = sqlx::query_scalar!(
            r#"SELECT MAX(rs.updated_at) as "max_updated_at"
            FROM release_states rs
            JOIN destinations d ON rs.destination_id = d.id
            WHERE rs.project_id = $1
              AND rs.artifact_id = $2
              AND d.environment = $3
              AND rs.status = 'SUCCEEDED'"#,
            project_id,
            artifact_id,
            source_env,
        )
        .fetch_one(&mut **tx)
        .await
        .context("check soak time in pipeline")?;

        match last_success {
            Some(ts) => {
                let elapsed = chrono::Utc::now() - ts;
                let required = chrono::Duration::seconds(duration_secs);
                if elapsed < required {
                    let remaining = (required - elapsed).num_seconds();
                    return Ok(Some(format!(
                        "policy '{}': {}s remaining ({}s elapsed, {}s required after {} deploy)",
                        policy.name,
                        remaining,
                        elapsed.num_seconds(),
                        duration_secs,
                        source_env,
                    )));
                }
            }
            None => {
                // No deploy to source env yet — soak time not applicable
                tracing::debug!(
                    policy = %policy.name,
                    source_env,
                    "no successful deploy to source env yet — soak time not applicable"
                );
            }
        }
    }

    Ok(None)
}
