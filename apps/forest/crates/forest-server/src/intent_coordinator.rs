use std::collections::HashMap;
use std::time::Duration;

use anyhow::Context;
use futures::StreamExt;
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::State;
use crate::services::release_event_store::{check_approval_policies, check_soak_time_policies};
use crate::services::release_pipeline::{
    ApprovalStatus, GateTimeout, PipelineStages, SignalRequirement, StageConfig, StageState,
    StageStates, StageStatus, find_ready_stages, has_failed_dependency, has_superseded_dependency,
    init_stage_states, is_pipeline_complete,
};

pub struct IntentCoordinator {
    state: State,
}

impl IntentCoordinator {
    pub fn new(state: &State) -> Self {
        Self {
            state: state.clone(),
        }
    }
}

impl Component for IntentCoordinator {
    fn info(&self) -> ComponentInfo {
        "forest-server/intent-coordinator".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        tracing::info!("intent coordinator starting");

        let mut nats_sub = self
            .state
            .nats
            .subscribe("forest.intent.evaluate")
            .await
            .map_err(|e| MadError::Inner(anyhow::anyhow!("NATS subscribe failed: {e}")))?;

        let mut sweep_interval = tokio::time::interval(Duration::from_secs(5));
        sweep_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        tracing::info!("intent coordinator ready, sweep interval=5s");

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => break,
                msg = nats_sub.next() => {
                    if let Some(msg) = msg {
                        let payload = String::from_utf8_lossy(&msg.payload);
                        if let Ok(intent_id) = payload.parse::<Uuid>() {
                            let state = self.state.clone();
                            tokio::spawn(async move {
                                if let Err(e) = evaluate(&state, intent_id).await {
                                    tracing::warn!(
                                        %intent_id,
                                        "intent coordinator: evaluate failed: {e:#}"
                                    );
                                }
                            });
                        }
                    }
                }
                _ = sweep_interval.tick() => {
                    let state = self.state.clone();
                    tokio::spawn(async move {
                        if let Err(e) = sweep_active_intents(&state).await {
                            tracing::error!("intent coordinator: sweep failed: {e:#}");
                        }
                    });
                }
            }
        }

        Ok(())
    }
}

async fn sweep_active_intents(state: &State) -> anyhow::Result<()> {
    let rows = sqlx::query_scalar!(
        r#"SELECT id as "id!"
         FROM release_intents
         WHERE status = 'ACTIVE'
           AND stages IS NOT NULL
           AND (next_evaluate_at IS NULL OR next_evaluate_at <= now())
         LIMIT 50"#,
    )
    .fetch_all(&state.db)
    .await?;

    if !rows.is_empty() {
        tracing::debug!(count = rows.len(), "intent coordinator sweep");
    }

    for intent_id in rows {
        if let Err(e) = evaluate(state, intent_id).await {
            tracing::warn!(%intent_id, "sweep: evaluate failed: {e:#}");
        }
    }

    Ok(())
}

pub async fn evaluate(state: &State, intent_id: Uuid) -> anyhow::Result<()> {
    let mut tx = state.db.begin().await?;

    let intent = sqlx::query!(
        "SELECT id, artifact, project_id, annotation_id, stages, stage_states, status
         FROM release_intents
         WHERE id = $1
         FOR UPDATE SKIP LOCKED",
        intent_id,
    )
    .fetch_optional(&mut *tx)
    .await?;

    let Some(intent) = intent else {
        return Ok(());
    };

    if intent.stages.is_none() || intent.status != "ACTIVE" {
        tx.commit().await?;
        return Ok(());
    }

    let stages: PipelineStages =
        serde_json::from_value(intent.stages.unwrap()).context("parse pipeline stages")?;

    let mut stage_states: StageStates = intent
        .stage_states
        .map(serde_json::from_value)
        .transpose()
        .context("parse stage_states")?
        .unwrap_or_else(|| init_stage_states(&stages));

    let release_rows = sqlx::query!(
        "SELECT release_id, stage_id, status, error_message
         FROM release_states
         WHERE release_intent_id = $1",
        intent_id,
    )
    .fetch_all(&mut *tx)
    .await?;

    let mut releases_by_stage: HashMap<String, Vec<ReleaseRow>> = HashMap::new();
    for row in &release_rows {
        if let Some(ref stage_id) = row.stage_id {
            releases_by_stage
                .entry(stage_id.clone())
                .or_default()
                .push(ReleaseRow {
                    status: row.status.clone(),
                    error_message: row.error_message.clone(),
                });
        }
    }

    let now = chrono::Utc::now();
    let now_str = now.to_rfc3339();
    let mut changed = false;
    let mut new_release_ids: Vec<Uuid> = Vec::new();
    let mut earliest_timer: Option<chrono::DateTime<chrono::Utc>> = None;

    let stage_ids_snapshot: Vec<String> = stage_states.keys().cloned().collect();
    for stage_id in &stage_ids_snapshot {
        let state_entry = stage_states.get(stage_id).cloned();
        let Some(ref current) = state_entry else {
            continue;
        };
        if current.status != StageStatus::Active {
            continue;
        }
        let Some(stage_def) = stages.get(stage_id) else {
            continue;
        };

        match &stage_def.config {
            StageConfig::Deploy { .. } => {
                let stage_releases = releases_by_stage.get(stage_id);
                let releases: &[ReleaseRow] = stage_releases.map(|v| v.as_slice()).unwrap_or(&[]);

                if releases.is_empty() {
                    continue;
                }

                let all_terminal = releases.iter().all(|r| {
                    matches!(
                        r.status.as_str(),
                        "SUCCEEDED" | "FAILED" | "CANCELLED" | "TIMED_OUT" | "SUPERSEDED"
                    )
                });
                if !all_terminal {
                    continue;
                }

                let mut updated = current.clone();
                updated.completed_at = Some(now_str.clone());

                match outcome_of(releases) {
                    StageOutcome::Succeeded => updated.status = StageStatus::Succeeded,
                    StageOutcome::Superseded => {
                        updated.status = StageStatus::Superseded;
                        updated.error_message = first_reason(releases);
                    }
                    StageOutcome::Failed => {
                        updated.status = StageStatus::Failed;
                        let errors: Vec<String> = releases
                            .iter()
                            .filter(|r| r.status != "SUCCEEDED" && r.status != "SUPERSEDED")
                            .filter_map(|r| r.error_message.clone())
                            .collect();
                        if !errors.is_empty() {
                            updated.error_message = Some(errors.join("; "));
                        }
                    }
                }
                stage_states.insert(stage_id.clone(), updated);
                changed = true;
            }
            StageConfig::Wait { .. } => {
                if let Some(ref wait_until_str) = current.wait_until
                    && let Ok(wait_until) = chrono::DateTime::parse_from_rfc3339(wait_until_str)
                {
                    let wait_until_utc = wait_until.with_timezone(&chrono::Utc);
                    if wait_until_utc <= now {
                        let mut updated = current.clone();
                        updated.status = StageStatus::Succeeded;
                        updated.completed_at = Some(now_str.clone());
                        stage_states.insert(stage_id.clone(), updated);
                        changed = true;
                    } else {
                        earliest_timer = Some(match earliest_timer {
                            Some(existing) => existing.min(wait_until_utc),
                            None => wait_until_utc,
                        });
                    }
                }
            }
            StageConfig::Gate {
                requires,
                on_timeout,
                ..
            } => {
                let signals =
                    crate::services::release_signals::list_for_intent(&state.db, intent_id)
                        .await
                        .unwrap_or_default();

                let unmet = unmet_requirements(requires, &signals);

                if unmet.is_empty() {
                    let mut updated = current.clone();
                    updated.status = StageStatus::Succeeded;
                    updated.completed_at = Some(now_str.clone());
                    updated.gate_waiting_on = None;
                    stage_states.insert(stage_id.clone(), updated);
                    changed = true;

                    tracing::info!(
                        %intent_id,
                        stage_id,
                        "coordinator: gate satisfied"
                    );
                    continue;
                }

                let deadline = current
                    .gate_deadline
                    .as_deref()
                    .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                    .map(|d| d.with_timezone(&chrono::Utc));

                match deadline {
                    Some(deadline) if deadline <= now => {
                        let mut updated = current.clone();
                        updated.completed_at = Some(now_str.clone());
                        updated.gate_waiting_on = Some(unmet.clone());
                        let waiting = unmet.join(", ");

                        match on_timeout {
                            GateTimeout::Proceed => {
                                updated.status = StageStatus::Succeeded;
                                updated.error_message = Some(format!(
                                    "gate timed out waiting for {waiting}; proceeding because \
                                     on_timeout is `proceed`"
                                ));
                                tracing::warn!(
                                    %intent_id,
                                    stage_id,
                                    "coordinator: gate timed out, proceeding: {waiting}"
                                );
                            }
                            GateTimeout::Fail => {
                                updated.status = StageStatus::Failed;
                                updated.error_message =
                                    Some(format!("gate timed out waiting for {waiting}"));
                                tracing::warn!(
                                    %intent_id,
                                    stage_id,
                                    "coordinator: gate timed out: {waiting}"
                                );
                            }
                        }

                        stage_states.insert(stage_id.clone(), updated);
                        changed = true;
                    }
                    _ => {
                        if current.gate_waiting_on.as_deref() != Some(unmet.as_slice()) {
                            let mut updated = current.clone();
                            updated.gate_waiting_on = Some(unmet);
                            stage_states.insert(stage_id.clone(), updated);
                            changed = true;
                        }
                        if let Some(deadline) = deadline {
                            earliest_timer = Some(match earliest_timer {
                                Some(existing) => existing.min(deadline),
                                None => deadline,
                            });
                        }
                    }
                }
            }
            StageConfig::Plan { auto_approve, .. } => {
                let stage_releases = releases_by_stage.get(stage_id);
                let releases: &[ReleaseRow] = stage_releases.map(|v| v.as_slice()).unwrap_or(&[]);

                if releases.is_empty() {
                    tracing::info!(%intent_id, stage_id, "coordinator: resetting empty plan stage to Pending for re-activation");
                    let mut updated = current.clone();
                    updated.status = StageStatus::Pending;
                    stage_states.insert(stage_id.clone(), updated);
                    changed = true;
                    continue;
                }

                let all_terminal = releases.iter().all(|r| {
                    matches!(
                        r.status.as_str(),
                        "SUCCEEDED" | "FAILED" | "CANCELLED" | "TIMED_OUT" | "SUPERSEDED"
                    )
                });
                if !all_terminal {
                    continue;
                }

                let outcome = outcome_of(releases);
                let mut updated = current.clone();

                if outcome == StageOutcome::Superseded {
                    updated.status = StageStatus::Superseded;
                    updated.completed_at = Some(now_str.clone());
                    updated.error_message = first_reason(releases);
                    stage_states.insert(stage_id.clone(), updated);
                    changed = true;
                } else if outcome == StageOutcome::Failed {
                    updated.status = StageStatus::Failed;
                    updated.completed_at = Some(now_str.clone());
                    let errors: Vec<String> = releases
                        .iter()
                        .filter(|r| r.status != "SUCCEEDED" && r.status != "SUPERSEDED")
                        .filter_map(|r| r.error_message.clone())
                        .collect();
                    if !errors.is_empty() {
                        updated.error_message = Some(errors.join("; "));
                    }
                    stage_states.insert(stage_id.clone(), updated);
                    changed = true;
                } else if *auto_approve {
                    updated.status = StageStatus::Succeeded;
                    updated.completed_at = Some(now_str.clone());
                    updated.approval_status = Some(ApprovalStatus::Approved);
                    updated.approval_at = Some(now_str.clone());
                    stage_states.insert(stage_id.clone(), updated);
                    changed = true;
                } else {
                    match current.approval_status {
                        Some(ApprovalStatus::Approved) => {
                            updated.status = StageStatus::Succeeded;
                            updated.completed_at = Some(now_str.clone());
                            stage_states.insert(stage_id.clone(), updated);
                            changed = true;
                        }
                        Some(ApprovalStatus::Rejected) => {
                            updated.status = StageStatus::Failed;
                            updated.completed_at = Some(now_str.clone());
                            updated.error_message = Some("plan rejected".into());
                            stage_states.insert(stage_id.clone(), updated);
                            changed = true;
                        }
                        _ => {
                            if current.approval_status.is_none() {
                                updated.approval_status = Some(ApprovalStatus::AwaitingApproval);
                                stage_states.insert(stage_id.clone(), updated);
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
    }

    let all_stage_ids: Vec<String> = stages.keys().cloned().collect();
    loop {
        let mut propagated = false;
        for stage_id in &all_stage_ids {
            let is_pending = stage_states
                .get(stage_id)
                .is_none_or(|s| s.status == StageStatus::Pending);
            if !is_pending {
                continue;
            }
            if has_failed_dependency(stage_id, &stages, &stage_states) {
                stage_states.insert(
                    stage_id.clone(),
                    StageState {
                        status: StageStatus::Cancelled,
                        error_message: Some("upstream stage failed".into()),
                        completed_at: Some(now_str.clone()),
                        ..StageState::pending()
                    },
                );
                propagated = true;
                changed = true;
            } else if has_superseded_dependency(stage_id, &stages, &stage_states) {
                stage_states.insert(
                    stage_id.clone(),
                    StageState {
                        status: StageStatus::Superseded,
                        error_message: Some("upstream stage superseded".into()),
                        completed_at: Some(now_str.clone()),
                        ..StageState::pending()
                    },
                );
                propagated = true;
                changed = true;
            }
        }
        if !propagated {
            break;
        }
    }

    let ready = find_ready_stages(&stages, &stage_states);

    for stage_id in &ready {
        let Some(stage_def) = stages.get(stage_id) else {
            continue;
        };

        match &stage_def.config {
            StageConfig::Deploy { environment } => {
                let soak_blocked = check_soak_time_policies(
                    &mut tx,
                    &intent.project_id,
                    &intent.artifact,
                    environment,
                )
                .await?;

                if let Some(reason) = soak_blocked {
                    tracing::debug!(
                        %intent_id,
                        stage_id,
                        environment,
                        "coordinator: deploy stage blocked by soak_time — {reason}"
                    );
                    let retry_at = now + chrono::Duration::seconds(30);
                    earliest_timer = Some(match earliest_timer {
                        Some(existing) => existing.min(retry_at),
                        None => retry_at,
                    });
                    continue;
                }

                let approval_blocked =
                    check_approval_policies(&mut tx, &intent.project_id, intent_id, environment)
                        .await?;
                if let Some(reason) = approval_blocked {
                    tracing::debug!(%intent_id, stage_id, environment, "coordinator: deploy stage blocked by approval — {reason}");
                    continue;
                }

                let resolved = resolve_stage_destinations(
                    &mut tx,
                    &intent.project_id,
                    &intent.annotation_id,
                    environment,
                )
                .await
                .context("resolve destinations for deploy stage")?;

                let dest_recs = match resolved {
                    StageResolution::Ready(destinations) => destinations,
                    StageResolution::Failed(error_message) => {
                        tracing::warn!(
                            %intent_id,
                            stage_id,
                            environment,
                            "coordinator: deploy stage failed — {error_message}"
                        );
                        stage_states.insert(
                            stage_id.clone(),
                            StageState {
                                status: StageStatus::Failed,
                                error_message: Some(error_message),
                                completed_at: Some(now_str.clone()),
                                ..StageState::pending()
                            },
                        );
                        changed = true;
                        continue;
                    }
                };

                let mut release_ids = Vec::new();
                for dest in &dest_recs {
                    let rid = Uuid::now_v7();
                    sqlx::query!(
                        "INSERT INTO release_states (
                            release_id, release_intent_id, project_id,
                            destination_id, artifact_id, status, stage_id
                        ) VALUES ($1, $2, $3, $4, $5, 'QUEUED', $6)",
                        rid,
                        intent_id,
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
                        queued_at: Some(now_str.clone()),
                        started_at: Some(now_str.clone()),
                        release_ids: Some(release_ids),
                        ..StageState::pending()
                    },
                );
                changed = true;

                tracing::info!(
                    %intent_id,
                    stage_id,
                    environment,
                    dest_count = dest_recs.len(),
                    "coordinator: activated deploy stage"
                );
            }
            StageConfig::Wait { duration_seconds } => {
                let wait_until = now + chrono::Duration::seconds(*duration_seconds);

                stage_states.insert(
                    stage_id.clone(),
                    StageState {
                        status: StageStatus::Active,
                        queued_at: Some(now_str.clone()),
                        started_at: Some(now_str.clone()),
                        wait_until: Some(wait_until.to_rfc3339()),
                        ..StageState::pending()
                    },
                );
                changed = true;

                earliest_timer = Some(match earliest_timer {
                    Some(existing) => existing.min(wait_until),
                    None => wait_until,
                });

                tracing::info!(
                    %intent_id,
                    stage_id,
                    duration_seconds,
                    "coordinator: activated wait stage (until {wait_until})"
                );
            }
            StageConfig::Gate {
                timeout_seconds, ..
            } => {
                let deadline = now + chrono::Duration::seconds(*timeout_seconds);

                stage_states.insert(
                    stage_id.clone(),
                    StageState {
                        status: StageStatus::Active,
                        queued_at: Some(now_str.clone()),
                        started_at: Some(now_str.clone()),
                        gate_deadline: Some(deadline.to_rfc3339()),
                        ..StageState::pending()
                    },
                );
                changed = true;

                earliest_timer = Some(match earliest_timer {
                    Some(existing) => existing.min(deadline),
                    None => deadline,
                });

                tracing::info!(
                    %intent_id,
                    stage_id,
                    timeout_seconds,
                    "coordinator: activated gate stage (deadline {deadline})"
                );
            }
            StageConfig::Plan { environment, .. } => {
                let soak_blocked = check_soak_time_policies(
                    &mut tx,
                    &intent.project_id,
                    &intent.artifact,
                    environment,
                )
                .await?;

                if let Some(reason) = soak_blocked {
                    tracing::debug!(
                        %intent_id,
                        stage_id,
                        environment,
                        "coordinator: plan stage blocked by soak_time — {reason}"
                    );
                    let retry_at = now + chrono::Duration::seconds(30);
                    earliest_timer = Some(match earliest_timer {
                        Some(existing) => existing.min(retry_at),
                        None => retry_at,
                    });
                    continue;
                }

                let resolved = resolve_stage_destinations(
                    &mut tx,
                    &intent.project_id,
                    &intent.annotation_id,
                    environment,
                )
                .await
                .context("resolve destinations for plan stage")?;

                let dest_recs = match resolved {
                    StageResolution::Ready(destinations) => destinations,
                    StageResolution::Failed(error_message) => {
                        tracing::warn!(
                            %intent_id,
                            stage_id,
                            environment,
                            "coordinator: plan stage failed — {error_message}"
                        );
                        stage_states.insert(
                            stage_id.clone(),
                            StageState {
                                status: StageStatus::Failed,
                                error_message: Some(error_message),
                                completed_at: Some(now_str.clone()),
                                ..StageState::pending()
                            },
                        );
                        changed = true;
                        continue;
                    }
                };

                let mut release_ids = Vec::new();
                for dest in &dest_recs {
                    let rid = Uuid::now_v7();
                    sqlx::query!(
                        "INSERT INTO release_states (
                            release_id, release_intent_id, project_id,
                            destination_id, artifact_id, status, stage_id, mode
                        ) VALUES ($1, $2, $3, $4, $5, 'QUEUED', $6, 'plan')",
                        rid,
                        intent_id,
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
                        queued_at: Some(now_str.clone()),
                        started_at: Some(now_str.clone()),
                        release_ids: Some(release_ids),
                        ..StageState::pending()
                    },
                );
                changed = true;

                tracing::info!(
                    %intent_id,
                    stage_id,
                    environment,
                    dest_count = dest_recs.len(),
                    "coordinator: activated plan stage"
                );
            }
        }
    }

    let pipeline_complete = is_pipeline_complete(&stage_states);
    let intent_status = if pipeline_complete {
        if stage_states
            .values()
            .all(|s| s.status == StageStatus::Succeeded)
        {
            "SUCCEEDED"
        } else if stage_states
            .values()
            .any(|s| matches!(s.status, StageStatus::Failed | StageStatus::Cancelled))
        {
            "FAILED"
        } else if stage_states
            .values()
            .any(|s| s.status == StageStatus::Superseded)
        {
            "SUPERSEDED"
        } else {
            "FAILED"
        }
    } else {
        "ACTIVE"
    };

    let stage_states_json = serde_json::to_value(&stage_states)?;
    let next_eval = if intent_status == "ACTIVE" {
        earliest_timer
    } else {
        None
    };

    sqlx::query!(
        "UPDATE release_intents
         SET stage_states = $2, status = $3, next_evaluate_at = $4
         WHERE id = $1",
        intent_id,
        stage_states_json,
        intent_status,
        next_eval,
    )
    .execute(&mut *tx)
    .await?;

    let org_project = sqlx::query!(
        "SELECT organisation, project FROM projects WHERE id = $1",
        intent.project_id,
    )
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(ref op) = org_project {
        use crate::services::event_bus::EventPayload;
        use std::collections::BTreeMap;

        for stage_id in stage_states.keys() {
            let Some(state_entry) = stage_states.get(stage_id) else {
                continue;
            };
            if state_entry.completed_at.as_deref() != Some(&now_str) {
                continue;
            }
            let status_str = match state_entry.status {
                StageStatus::Succeeded => "SUCCEEDED",
                StageStatus::Failed => "FAILED",
                StageStatus::Cancelled => "CANCELLED",
                StageStatus::Superseded => "SUPERSEDED",
                _ => continue,
            };
            let mut meta = BTreeMap::new();
            meta.insert("intent_id".into(), intent_id.to_string());
            meta.insert("stage_id".into(), stage_id.clone());
            meta.insert("stage_status".into(), status_str.into());

            crate::services::event_bus::EventBus::record(
                &mut tx,
                EventPayload {
                    organisation: op.organisation.clone(),
                    project: op.project.clone(),
                    resource_type: "pipeline",
                    action: "stage_changed",
                    resource_id: intent_id.to_string(),
                    metadata: meta,
                },
            )
            .await?;
        }

        if pipeline_complete {
            let mut meta = BTreeMap::new();
            meta.insert("intent_id".into(), intent_id.to_string());
            meta.insert("pipeline_status".into(), intent_status.into());

            crate::services::event_bus::EventBus::record(
                &mut tx,
                EventPayload {
                    organisation: op.organisation.clone(),
                    project: op.project.clone(),
                    resource_type: "pipeline",
                    action: "completed",
                    resource_id: intent_id.to_string(),
                    metadata: meta,
                },
            )
            .await?;
        }
    }

    tx.commit().await?;

    for rid in &new_release_ids {
        let _ = state
            .nats
            .publish("forest.release.queued", rid.to_string().into())
            .await;
    }

    if changed {
        let nats_subject = format!("forest.release.status.{}", intent_id);
        let nats_payload = serde_json::json!({
            "pipeline_update": true,
            "pipeline_complete": pipeline_complete,
            "intent_status": intent_status,
        });
        let _ = state
            .nats
            .publish(nats_subject, nats_payload.to_string().into())
            .await;
    }

    if let Some(ref op) = org_project {
        let org_subject = format!("forest.events.{}", op.organisation);
        let _ = state.nats.publish(org_subject, "".into()).await;
    }

    if pipeline_complete {
        tracing::info!(%intent_id, status = intent_status, "coordinator: pipeline complete");
    } else if changed {
        let _ = state
            .nats
            .publish("forest.intent.evaluate", intent_id.to_string().into())
            .await;
    } else if let Some(timer) = earliest_timer {
        let state = state.clone();
        let delay = (timer - chrono::Utc::now())
            .to_std()
            .unwrap_or(Duration::from_millis(100));
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = state
                .nats
                .publish("forest.intent.evaluate", intent_id.to_string().into())
                .await;
        });
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StageOutcome {
    Succeeded,
    Failed,
    Superseded,
}

fn outcome_of(releases: &[ReleaseRow]) -> StageOutcome {
    if releases.iter().all(|r| r.status == "SUCCEEDED") {
        return StageOutcome::Succeeded;
    }
    let any_failed = releases
        .iter()
        .any(|r| matches!(r.status.as_str(), "FAILED" | "CANCELLED" | "TIMED_OUT"));
    if any_failed {
        return StageOutcome::Failed;
    }
    if releases.iter().any(|r| r.status == "SUPERSEDED") {
        return StageOutcome::Superseded;
    }
    StageOutcome::Failed
}

fn first_reason(releases: &[ReleaseRow]) -> Option<String> {
    releases
        .iter()
        .find(|r| r.status == "SUPERSEDED")
        .and_then(|r| r.error_message.clone())
}

struct StageDestination {
    id: Uuid,
    name: String,
    destination_type: String,
}

enum StageResolution {
    Ready(Vec<StageDestination>),
    Failed(String),
}

fn unmet_requirements(
    requires: &[SignalRequirement],
    signals: &[crate::services::release_signals::SignalRow],
) -> Vec<String> {
    requires
        .iter()
        .filter_map(|req| {
            let accepted = req.accepted();
            let satisfied = signals
                .iter()
                .any(|s| s.name == req.signal && accepted.contains(&s.status));
            if satisfied {
                return None;
            }

            let seen: Vec<&str> = signals
                .iter()
                .filter(|s| s.name == req.signal)
                .map(|s| s.status.as_str())
                .collect();
            Some(if seen.is_empty() {
                format!(
                    "{} to be {} (nothing reported)",
                    req.signal,
                    accepted.join(" or ")
                )
            } else {
                format!(
                    "{} to be {} (currently {})",
                    req.signal,
                    accepted.join(" or "),
                    seen.join(", ")
                )
            })
        })
        .collect()
}

async fn resolve_stage_destinations(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: &Uuid,
    annotation_id: &Uuid,
    environment: &str,
) -> anyhow::Result<StageResolution> {
    use crate::services::destination_selector;

    let in_environment = sqlx::query!(
        r#"SELECT d.id, d.name, d.type_organisation, d.type_name, d.type_version
         FROM destinations d
         JOIN environments e ON d.environment_id = e.id
         JOIN projects p ON p.id = $2
         WHERE e.name = $1
           AND e.organisation = p.organisation
           AND d.organisation = p.organisation"#,
        environment,
        project_id,
    )
    .fetch_all(&mut **tx)
    .await
    .context("resolve environment to destinations")?;

    if in_environment.is_empty() {
        return Ok(StageResolution::Failed(format!(
            "no destinations configured for environment '{environment}'"
        )));
    }

    let candidates: Vec<StageDestination> = in_environment
        .into_iter()
        .map(|d| StageDestination {
            id: d.id,
            name: d.name,
            destination_type: format!("{}/{}@{}", d.type_organisation, d.type_name, d.type_version),
        })
        .collect();

    let declared = sqlx::query_scalar!(
        "SELECT deployment_items FROM annotations WHERE id = $1",
        annotation_id,
    )
    .fetch_optional(&mut **tx)
    .await
    .context("read the project's declaration for this release")?
    .flatten();

    let declared = declared.filter(|value| !value.is_null());

    let items: Vec<destination_selector::DeploymentItem> = match declared {
        Some(value) => match serde_json::from_value(value) {
            Ok(items) => items,
            Err(e) => {
                return Ok(StageResolution::Failed(format!(
                    "this release's recorded declaration could not be read ({e}), so which destinations in '{environment}' it asked for is unknown"
                )));
            }
        },
        None => Vec::new(),
    };

    match destination_selector::narrow_to_declared(candidates, &items, environment, |dest| {
        (dest.name.as_str(), dest.destination_type.as_str())
    }) {
        Ok(selected) => Ok(StageResolution::Ready(selected)),
        Err(message) => Ok(StageResolution::Failed(message)),
    }
}

struct ReleaseRow {
    status: String,
    error_message: Option<String>,
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use crate::services::release_signals::SignalRow;

    fn signal(name: &str, status: &str) -> SignalRow {
        SignalRow {
            name: name.to_string(),
            status: status.to_string(),
            detail: String::new(),
            destination_name: "platform-dev/eu-west-1/infrastructure-platform".to_string(),
            environment: "platform-dev".to_string(),
            reported_by: "forest-ecs-provider".to_string(),
            observed_at: chrono::Utc::now(),
        }
    }

    fn req(name: &str, accept: &[&str]) -> SignalRequirement {
        SignalRequirement {
            signal: name.to_string(),
            accept: accept.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn a_reported_healthy_signal_satisfies_the_gate() {
        let unmet = unmet_requirements(
            &[req("rollout", &["HEALTHY"])],
            &[signal("rollout", "HEALTHY")],
        );
        assert!(unmet.is_empty(), "got: {unmet:?}");
    }

    #[test]
    fn an_unhealthy_signal_does_not_satisfy_a_gate_waiting_for_healthy() {
        let unmet = unmet_requirements(
            &[req("rollout", &["HEALTHY"])],
            &[signal("rollout", "UNHEALTHY")],
        );
        assert_eq!(unmet.len(), 1);
        assert!(unmet[0].contains("currently UNHEALTHY"), "got: {unmet:?}");
    }

    #[test]
    fn nothing_reported_says_so_rather_than_naming_a_state() {
        let unmet = unmet_requirements(&[req("rollout", &["HEALTHY"])], &[]);
        assert_eq!(unmet.len(), 1);
        assert!(unmet[0].contains("nothing reported"), "got: {unmet:?}");
        assert!(unmet[0].contains("rollout"), "got: {unmet:?}");
    }

    #[test]
    fn a_different_signal_does_not_satisfy_the_requirement() {
        let unmet = unmet_requirements(
            &[req("rollout", &["HEALTHY"])],
            &[signal("smoke", "HEALTHY")],
        );
        assert_eq!(unmet.len(), 1);
        assert!(unmet[0].contains("nothing reported"), "got: {unmet:?}");
    }

    #[test]
    fn any_of_the_accepted_states_satisfies_it() {
        for status in ["HEALTHY", "DEGRADED"] {
            let unmet = unmet_requirements(
                &[req("rollout", &["HEALTHY", "DEGRADED"])],
                &[signal("rollout", status)],
            );
            assert!(unmet.is_empty(), "{status} should satisfy: {unmet:?}");
        }
        let unmet = unmet_requirements(
            &[req("rollout", &["HEALTHY", "DEGRADED"])],
            &[signal("rollout", "UNHEALTHY")],
        );
        assert_eq!(unmet.len(), 1, "UNHEALTHY should not: {unmet:?}");
    }

    #[test]
    fn all_requirements_must_be_satisfied() {
        let requires = [req("rollout", &["HEALTHY"]), req("smoke", &["HEALTHY"])];
        let unmet = unmet_requirements(&requires, &[signal("rollout", "HEALTHY")]);
        assert_eq!(unmet.len(), 1);
        assert!(unmet[0].contains("smoke"), "got: {unmet:?}");

        let unmet = unmet_requirements(
            &requires,
            &[signal("rollout", "HEALTHY"), signal("smoke", "HEALTHY")],
        );
        assert!(unmet.is_empty(), "got: {unmet:?}");
    }

    #[test]
    fn one_destination_reporting_is_enough() {
        let mut from_other = signal("rollout", "HEALTHY");
        from_other.destination_name = "finance/eu-west-1/infrastructure-finance".to_string();
        let unmet = unmet_requirements(&[req("rollout", &["HEALTHY"])], &[from_other]);
        assert!(unmet.is_empty(), "got: {unmet:?}");
    }

    #[test]
    fn an_omitted_accept_list_waits_for_healthy() {
        let unmet = unmet_requirements(&[req("rollout", &[])], &[signal("rollout", "DEGRADED")]);
        assert_eq!(unmet.len(), 1);
        assert!(unmet[0].contains("to be HEALTHY"), "got: {unmet:?}");
    }
}
