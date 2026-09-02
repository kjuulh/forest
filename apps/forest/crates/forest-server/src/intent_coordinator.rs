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
    ApprovalStatus, PipelineStages, StageConfig, StageState, StageStates, StageStatus,
    find_ready_stages, has_failed_dependency, init_stage_states, is_pipeline_complete,
};

/// The IntentCoordinator is the single saga orchestrator for pipeline release intents.
///
/// It owns the full lifecycle of a pipeline: activating stages, completing wait stages,
/// propagating cancellations, enforcing soak_time policies, and marking the intent as
/// SUCCEEDED or FAILED when all stages are terminal.
///
/// Wake-up signals:
///   - NATS `forest.intent.evaluate` (published when a release finishes, or a new intent is created)
///   - 5-second periodic sweep (crash recovery, timer expiry, soak_time retry)
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

/// Sweep all ACTIVE pipeline intents that are due for evaluation.
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

/// The core idempotent evaluation function.
///
/// Loads the full state of a release intent (stages, stage_states, child release_states),
/// walks the DAG holistically, and takes all possible actions in a single transaction:
///   - Derive ACTIVE deploy stage status from child releases
///   - Complete expired wait stages
///   - Propagate cancellations transitively
///   - Activate PENDING stages whose deps are satisfied (with soak_time checks)
///   - Compute intent-level terminal status
///
/// Public so acceptance tests can drive exactly one evaluation and then assert
/// on rows. The test fixture runs the gRPC server and the `Scheduler` but not
/// this component: it is process-wide across every acceptance test, and a
/// background loop mutating every intent in the shared dev database would make
/// unrelated tests flaky. A direct call needs no sleeping and cannot race.
pub async fn evaluate(state: &State, intent_id: Uuid) -> anyhow::Result<()> {
    let mut tx = state.db.begin().await?;

    // Step 1: Lock the intent
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
        return Ok(()); // Intent doesn't exist or locked by another evaluator
    };

    // Skip non-pipeline intents and already-terminal intents
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

    // Step 2: Load all release_states for this intent
    let release_rows = sqlx::query!(
        "SELECT release_id, stage_id, status, error_message
         FROM release_states
         WHERE release_intent_id = $1",
        intent_id,
    )
    .fetch_all(&mut *tx)
    .await?;

    // Group by stage_id
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

    // Step 3a: Derive status of ACTIVE stages from their children
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
                    continue; // No releases yet (shouldn't happen for ACTIVE deploy)
                }

                let all_terminal = releases.iter().all(|r| {
                    matches!(
                        r.status.as_str(),
                        "SUCCEEDED" | "FAILED" | "CANCELLED" | "TIMED_OUT"
                    )
                });
                if !all_terminal {
                    continue; // Still in progress
                }

                let all_succeeded = releases.iter().all(|r| r.status == "SUCCEEDED");

                let mut updated = current.clone();
                if all_succeeded {
                    updated.status = StageStatus::Succeeded;
                    updated.completed_at = Some(now_str.clone());
                } else {
                    updated.status = StageStatus::Failed;
                    updated.completed_at = Some(now_str.clone());
                    // Aggregate error messages from failed releases
                    let errors: Vec<String> = releases
                        .iter()
                        .filter(|r| r.status != "SUCCEEDED")
                        .filter_map(|r| r.error_message.clone())
                        .collect();
                    if !errors.is_empty() {
                        updated.error_message = Some(errors.join("; "));
                    }
                }
                stage_states.insert(stage_id.clone(), updated);
                changed = true;
            }
            StageConfig::Wait { .. } => {
                // Check if wait_until has passed
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
                        // Track earliest timer for next_evaluate_at
                        earliest_timer = Some(match earliest_timer {
                            Some(existing) => existing.min(wait_until_utc),
                            None => wait_until_utc,
                        });
                    }
                }
            }
            StageConfig::Plan { auto_approve, .. } => {
                // Plan stages work like deploy stages but with an approval gate
                let stage_releases = releases_by_stage.get(stage_id);
                let releases: &[ReleaseRow] = stage_releases.map(|v| v.as_slice()).unwrap_or(&[]);

                if releases.is_empty() {
                    // ACTIVE plan stage with no child releases — this can happen if
                    // activation was blocked after setting status=Active. Reset to
                    // Pending so Step 4 re-attempts activation.
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
                        "SUCCEEDED" | "FAILED" | "CANCELLED" | "TIMED_OUT"
                    )
                });
                if !all_terminal {
                    continue;
                }

                let all_succeeded = releases.iter().all(|r| r.status == "SUCCEEDED");
                let mut updated = current.clone();

                if !all_succeeded {
                    // Plan execution itself failed
                    updated.status = StageStatus::Failed;
                    updated.completed_at = Some(now_str.clone());
                    let errors: Vec<String> = releases
                        .iter()
                        .filter(|r| r.status != "SUCCEEDED")
                        .filter_map(|r| r.error_message.clone())
                        .collect();
                    if !errors.is_empty() {
                        updated.error_message = Some(errors.join("; "));
                    }
                    stage_states.insert(stage_id.clone(), updated);
                    changed = true;
                } else if *auto_approve {
                    // Auto-approve: plan succeeded, skip approval gate
                    updated.status = StageStatus::Succeeded;
                    updated.completed_at = Some(now_str.clone());
                    updated.approval_status = Some(ApprovalStatus::Approved);
                    updated.approval_at = Some(now_str.clone());
                    stage_states.insert(stage_id.clone(), updated);
                    changed = true;
                } else {
                    // Manual approval required
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
                            // Plan succeeded but no approval yet — set awaiting
                            if current.approval_status.is_none() {
                                updated.approval_status = Some(ApprovalStatus::AwaitingApproval);
                                stage_states.insert(stage_id.clone(), updated);
                                changed = true;
                            }
                            // Stay Active, don't complete
                        }
                    }
                }
            }
        }
    }

    // Step 3b: Propagate cancellations transitively
    let all_stage_ids: Vec<String> = stages.keys().cloned().collect();
    loop {
        let mut propagated = false;
        for stage_id in &all_stage_ids {
            let is_pending = stage_states
                .get(stage_id)
                .is_none_or(|s| s.status == StageStatus::Pending);
            if is_pending && has_failed_dependency(stage_id, &stages, &stage_states) {
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
            }
        }
        if !propagated {
            break;
        }
    }

    // Step 3c: Find PENDING stages whose deps are all SUCCEEDED
    let ready = find_ready_stages(&stages, &stage_states);

    for stage_id in &ready {
        let Some(stage_def) = stages.get(stage_id) else {
            continue;
        };

        match &stage_def.config {
            StageConfig::Deploy { environment } => {
                // Check soak_time policies inside the transaction
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
                    // Schedule retry — use a conservative 30s or parse remaining from reason
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
            StageConfig::Plan { environment, .. } => {
                // Plan stages work like deploy but create releases in plan mode
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

                // Plan stages skip external approval checks — the plan is a dry-run
                // that should execute so users can review the output before approving.
                // The plan stage has its own built-in approval gate (AWAITING_APPROVAL).

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

    // Step 4: Compute intent-level status
    let pipeline_complete = is_pipeline_complete(&stage_states);
    let intent_status = if pipeline_complete {
        if stage_states
            .values()
            .all(|s| s.status == StageStatus::Succeeded)
        {
            "SUCCEEDED"
        } else {
            "FAILED"
        }
    } else {
        "ACTIVE"
    };

    // Step 5: Persist updated state
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

    // Write org_events inside the transaction
    let org_project = sqlx::query!(
        "SELECT organisation, project FROM projects WHERE id = $1",
        intent.project_id,
    )
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(ref op) = org_project {
        use crate::services::event_bus::EventPayload;
        use std::collections::BTreeMap;

        // Event for each stage that just completed in this evaluation
        for stage_id in stage_states.keys() {
            let Some(state_entry) = stage_states.get(stage_id) else {
                continue;
            };
            // Only emit for stages that completed *in this evaluation*
            if state_entry.completed_at.as_deref() != Some(&now_str) {
                continue;
            }
            let status_str = match state_entry.status {
                StageStatus::Succeeded => "SUCCEEDED",
                StageStatus::Failed => "FAILED",
                StageStatus::Cancelled => "CANCELLED",
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

        // Pipeline completion event
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

    // Step 6: Commit
    tx.commit().await?;

    // Step 7: After-commit NATS signals
    // Signal newly queued releases to the scheduler
    for rid in &new_release_ids {
        let _ = state
            .nats
            .publish("forest.release.queued", rid.to_string().into())
            .await;
    }

    // Publish pipeline status update for WaitRelease stream
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

    // Nudge org event listeners
    if let Some(ref op) = org_project {
        let org_subject = format!("forest.events.{}", op.organisation);
        let _ = state.nats.publish(org_subject, "".into()).await;
    }

    if pipeline_complete {
        tracing::info!(%intent_id, status = intent_status, "coordinator: pipeline complete");
    } else if changed {
        // Pipeline still active and we made progress — re-evaluate immediately.
        // This handles cascading stage transitions (e.g. wait completes → deploy activates)
        // without waiting for the 5s sweep.
        let _ = state
            .nats
            .publish("forest.intent.evaluate", intent_id.to_string().into())
            .await;
    } else if let Some(timer) = earliest_timer {
        // No progress but we have a pending timer (wait stage or soak_time retry).
        // Spawn a delayed re-evaluation to fire precisely when the timer expires.
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

/// A destination a stage is about to release to.
struct StageDestination {
    id: Uuid,
    name: String,
    /// `organisation/name@version`, as `destination create --type` spells it.
    /// Carried because selection compares it against the type each declared
    /// item names — see `destination_selector::selects`.
    destination_type: String,
}

/// The outcome of working out where a stage should release to.
enum StageResolution {
    Ready(Vec<StageDestination>),
    /// The stage cannot run, and this says why in terms the person reading the
    /// release can act on.
    Failed(String),
}

/// Which destinations a deploy or plan stage releases to.
///
/// Two filters, in order.
///
/// **The organisation.** Environment names are globally non-unique, so without
/// this a `dev` stage would fan out into every organisation's `dev`
/// destinations.
///
/// **The project's declaration.** A stage names an environment; the project's
/// `forest.cue` names the destinations *within* an environment it releases to,
/// as selectors, each with the destination *type* it renders for. Resolving the
/// environment and stopping there is what scheduled an ECS service artifact at a
/// shiitake slice registry: the environment held both, the project had asked for
/// one of them, and nothing consulted the ask. The declaration was recorded on
/// the annotation at annotate time — see `destination_selector` — so this works
/// identically for a release fired by a trigger, where no client supplied
/// anything.
///
/// The type half is the second turn of the same screw. Honouring the selector
/// but not the type still schedules work nobody declared: `fungus` declares
/// `forest/terraform@1` for `^dev/.*$` and `forest/generic@1` for
/// `^platform-dev/.*$`, and both selectors reached both destinations in
/// `platform-dev` — so a terraform plan ran at an ECS place with no terraform in
/// the artifact to run. A destination whose type this project declares nothing
/// for is not scheduled at all; it is out of scope, not failed.
///
/// Declaring nothing for the stage's environment means no filtering, not an
/// empty filter. Projects that name no destinations must keep releasing to
/// whole environments or this fix breaks every one of them to help one.
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

    // Absent for artifacts annotated before the column existed, which read as
    // "nothing declared" and therefore fan out — exactly what they do today.
    let declared = sqlx::query_scalar!(
        "SELECT deployment_items FROM annotations WHERE id = $1",
        annotation_id,
    )
    .fetch_optional(&mut **tx)
    .await
    .context("read the project's declaration for this release")?
    .flatten();

    // A JSON `null` reads as "nothing recorded", the same as a SQL NULL. Our
    // writer only ever stores an array, but the two spellings of absent should
    // not behave differently — one of them failing a stage would be a puzzle.
    let declared = declared.filter(|value| !value.is_null());

    let items: Vec<destination_selector::DeploymentItem> = match declared {
        // Fail the stage rather than the evaluation. Propagating this error would
        // leave the stage PENDING and let the 5s sweep retry it forever — a
        // release that hangs silently instead of one that says what is wrong.
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

    // The rule itself lives in `destination_selector`, shared with the request
    // path. Two copies is how the pipeline path came to ignore declarations the
    // request path already honoured.
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
