use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{State, actor::Actor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseEventType {
    Requested,
    Assigned,
    Started,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Superseded,
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
            Self::Superseded => "release.superseded",
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
            Self::Superseded => "SUPERSEDED",
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
            Self::Superseded => &["QUEUED"],
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
    pub stage_id: Option<String>,
}

#[derive(Clone)]
pub struct ReleaseEventStore {
    pub(crate) db: PgPool,
    pub(crate) nats: async_nats::Client,
}

impl ReleaseEventStore {
    pub async fn create_release(&self, params: CreateReleaseParams) -> anyhow::Result<Uuid> {
        let release_id = Uuid::now_v7();
        let mut tx = self.db.begin().await?;

        let actor_id = params.actor.actor_id();
        let actor_type = params.actor.actor_type();

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

        for (cancelled_id, cancelled_intent_id) in &cancelled_intent_ids {
            let subject = format!("forest.release.status.{}", cancelled_intent_id);
            let payload = serde_json::json!({
                "release_id": cancelled_id.to_string(),
                "status": "CANCELLED",
            });
            let _ = self.nats.publish(subject, payload.to_string().into()).await;
        }

        if let Err(e) = self
            .nats
            .publish("forest.release.queued", release_id.to_string().into())
            .await
        {
            tracing::warn!("failed to publish release.queued to NATS: {e}");
        }

        Ok(release_id)
    }

    pub async fn create_failed_release(
        &self,
        params: CreateReleaseParams,
        error_message: &str,
    ) -> anyhow::Result<Uuid> {
        let release_id = Uuid::now_v7();
        let mut tx = self.db.begin().await?;

        let actor_id = params.actor.actor_id();
        let actor_type = params.actor.actor_type();

        sqlx::query!(
            "INSERT INTO release_states (
                release_id, release_intent_id, project_id,
                destination_id, artifact_id, status, stage_id,
                error_message, completed_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, 'FAILED', $6, $7, now(), now())",
            release_id,
            params.release_intent_id,
            params.project_id,
            params.destination_id,
            params.artifact_id,
            params.stage_id,
            error_message,
        )
        .execute(&mut *tx)
        .await
        .context("insert failed release_states")?;

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

        sqlx::query!(
            "INSERT INTO release_events (
                release_id, event_type, payload, actor_id, actor_type
            ) VALUES ($1, 'release.failed', $2, $3, $4)",
            release_id,
            serde_json::json!({ "error_message": error_message }),
            actor_id,
            actor_type,
        )
        .execute(&mut *tx)
        .await
        .context("insert release.failed event")?;

        tx.commit().await?;

        let subject = format!("forest.release.status.{}", params.release_intent_id);
        let payload = serde_json::json!({
            "release_id": release_id.to_string(),
            "status": "FAILED",
        });
        let _ = self.nats.publish(subject, payload.to_string().into()).await;

        Ok(release_id)
    }

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
            ReleaseEventType::Superseded => {
                sqlx::query!(
                    "UPDATE release_states SET
                        status = $2, error_message = $3,
                        completed_at = now(), updated_at = now()
                     WHERE release_id = $1",
                    release_id,
                    target_status,
                    payload.reason,
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

        if let Some(ref op) = org_project {
            let org_subject = format!("forest.events.{}", op.organisation);
            let _ = self.nats.publish(org_subject, "".into()).await;
        }

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

            if stage_id.is_some() {
                let _ = self
                    .nats
                    .publish(
                        "forest.intent.evaluate",
                        release_intent_id.to_string().into(),
                    )
                    .await;
            } else {
                self.try_finalize_direct_intent(&release_intent_id).await;
            }
        }

        Ok(())
    }

    pub async fn find_orphaned_active_intents(
        &self,
        min_age_secs: i64,
    ) -> anyhow::Result<Vec<Uuid>> {
        let ids = sqlx::query_scalar!(
            r#"
            SELECT ri.id AS "id!"
            FROM release_intents ri
            WHERE ri.status = 'ACTIVE'
              AND ri.stages IS NULL
              AND ri.created < now() - make_interval(secs => $1::double precision)
              AND EXISTS (
                    SELECT 1 FROM release_states rs
                    WHERE rs.release_intent_id = ri.id
              )
              AND NOT EXISTS (
                    SELECT 1 FROM release_states rs
                    WHERE rs.release_intent_id = ri.id
                      AND rs.status NOT IN ('SUCCEEDED', 'FAILED', 'CANCELLED', 'TIMED_OUT', 'SUPERSEDED')
              )
            "#,
            min_age_secs as f64,
        )
        .fetch_all(&self.db)
        .await
        .context("find orphaned active intents")?;

        Ok(ids)
    }

    pub(crate) async fn try_finalize_direct_intent(&self, intent_id: &Uuid) {
        let result: Result<_, anyhow::Error> = async {
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

            let non_terminal = sqlx::query_scalar!(
                r#"SELECT count(*) as "count!" FROM release_states
                 WHERE release_intent_id = $1
                   AND status NOT IN ('SUCCEEDED', 'FAILED', 'CANCELLED', 'TIMED_OUT', 'SUPERSEDED')"#,
                intent_id,
            )
            .fetch_one(&self.db)
            .await?;

            if non_terminal > 0 {
                return Ok(());
            }

            let any_failed = sqlx::query_scalar!(
                r#"SELECT count(*) as "count!" FROM release_states
                 WHERE release_intent_id = $1
                   AND status IN ('FAILED', 'TIMED_OUT')"#,
                intent_id,
            )
            .fetch_one(&self.db)
            .await?;

            let intent_status = if any_failed > 0 {
                "FAILED"
            } else {
                "SUCCEEDED"
            };

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

    const ORG_INTENT_LIMIT: i64 = 50;

    pub async fn get_release_intent_states(
        &self,
        organisation: &str,
        project_id: Option<&Uuid>,
        include_completed: bool,
    ) -> anyhow::Result<Vec<(ReleaseIntentRow, Vec<ReleaseStepRow>)>> {
        let limit: Option<i64> = match project_id {
            Some(_) => None,
            None => Some(Self::ORG_INTENT_LIMIT),
        };
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
            LIMIT $4::bigint"#,
            organisation,
            project_id,
            include_completed,
            limit,
        )
        .fetch_all(&self.db)
        .await
        .context("get release intent states")?;

        if intents.is_empty() {
            return Ok(Vec::new());
        }

        let intent_ids: Vec<Uuid> = intents.iter().map(|i| i.release_intent_id).collect();

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
        let target = config
            .get("target_environment")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if target != target_environment {
            continue;
        }
        let required = config
            .get("required_approvals")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);

        let approved_count = sqlx::query_scalar!(
            r#"SELECT COUNT(*) as "count!" FROM approval_decisions
             WHERE release_intent_id = $1 AND target_environment = $2 AND decision = 'approved'"#,
            release_intent_id,
            target_environment,
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

pub(crate) async fn check_branch_policies(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: &Uuid,
    annotation_id: &Uuid,
    target_environment: &str,
) -> anyhow::Result<Option<String>> {
    let policies = sqlx::query!(
        r#"SELECT name, config
        FROM policies
        WHERE project_id = $1
          AND enabled = true
          AND policy_type = 'branch_restriction'"#,
        project_id,
    )
    .fetch_all(&mut **tx)
    .await
    .context("load branch_restriction policies")?;
    if policies.is_empty() {
        return Ok(None);
    }

    let branch = sqlx::query_scalar!(
        r#"SELECT ref->>'commit_branch' as "branch" FROM annotations WHERE id = $1"#,
        annotation_id,
    )
    .fetch_optional(&mut **tx)
    .await
    .context("load the annotation's branch")?
    .flatten();

    for policy in policies {
        let crate::services::policy::PolicyConfig::BranchRestriction(config) =
            crate::services::policy::PolicyConfig::from_record(
                "branch_restriction",
                &policy.config,
            )?
        else {
            continue;
        };
        if config.target_environment != target_environment {
            continue;
        }
        let eval = crate::services::policy::branch_restriction_verdict(
            &config,
            branch.as_deref(),
            &policy.name,
        );
        if !eval.passed {
            return Ok(Some(format!(
                "blocked by policy '{}': {}",
                policy.name, eval.reason
            )));
        }
    }

    Ok(None)
}

pub(crate) async fn check_soak_time_policies(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: &Uuid,
    artifact_id: &Uuid,
    target_environment: &str,
) -> anyhow::Result<Option<String>> {
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
        let crate::services::policy::PolicyConfig::SoakTime(config) =
            crate::services::policy::PolicyConfig::from_record("soak_time", &policy.config)?
        else {
            continue;
        };
        if config.target_environment != target_environment {
            continue;
        }

        let last_deploy = crate::services::policy::last_source_deploy(
            tx,
            project_id,
            Some(artifact_id),
            &config.source_environment,
        )
        .await
        .context("check soak time in pipeline")?;
        let (passed, reason) =
            crate::services::policy::soak_verdict(last_deploy, chrono::Utc::now(), &config, true);
        if !passed {
            return Ok(Some(format!("policy '{}': {reason}", policy.name)));
        }
    }

    Ok(None)
}
