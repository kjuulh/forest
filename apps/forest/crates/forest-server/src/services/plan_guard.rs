use std::time::UNIX_EPOCH;

use anyhow::Context;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::destinations::terraform_plans::PLAN_RETENTION_DAYS;
use crate::services::policy::{PolicyRegistry, SupersedePendingConfig};
use crate::services::release_event_store::ReleaseState;
use crate::services::release_logs_registry::{LogChannel, LogLine, ReleaseLogsRegistry};
use crate::services::release_pipeline::{
    ApprovalStatus, PipelineStages, StageConfig, StageState, StageStates, StageStatus,
};

#[derive(Debug, Clone)]
pub struct HolderRelease {
    pub release_id: Uuid,
    pub stage_id: Option<String>,
    pub mode: String,
    pub status: String,
    pub queued_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PlanHolder {
    pub intent_id: Uuid,
    pub project_id: Uuid,
    pub slug: String,
    pub branch: Option<String>,
    pub intent_status: String,
    pub has_saved_plan: bool,
    pub saved_plan_changes: bool,
    pub stages: PipelineStages,
    pub stage_states: StageStates,
    pub releases: Vec<HolderRelease>,
}

#[derive(Debug, Clone, Copy)]
pub struct WaitingPlan {
    pub intent_id: Uuid,
    pub project_id: Uuid,
    pub release_id: Uuid,
    pub queued_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    SavedPlan,
    PlanningAhead,
}

#[derive(Debug)]
pub enum Decision<'a> {
    Wait(&'a PlanHolder, Hold),
    Proceed {
        retire: Vec<(&'a PlanHolder, Vec<String>)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Retirement {
    Superseded { policy: String },
    Stale,
}

fn is_terminal_release(status: &str) -> bool {
    matches!(
        status,
        "SUCCEEDED" | "FAILED" | "CANCELLED" | "TIMED_OUT" | "SUPERSEDED"
    )
}

/// A generic destination saves a plan when its provider seals one; until it
/// does, only plans still in flight hold.
pub fn saves_plans(type_organisation: &str, type_name: &str) -> bool {
    type_organisation == "forest" && matches!(type_name, "terraform" | "generic")
}

fn is_approved(config: &StageConfig, state: &StageState) -> bool {
    let auto_approve = matches!(
        config,
        StageConfig::Plan {
            auto_approve: true,
            ..
        }
    );
    match state.approval_status {
        Some(ApprovalStatus::Approved) => true,
        None => auto_approve,
        Some(ApprovalStatus::AwaitingApproval | ApprovalStatus::Rejected) => false,
    }
}

impl PlanHolder {
    fn stage_releases<'a>(&'a self, stage_id: &'a str) -> impl Iterator<Item = &'a HolderRelease> {
        self.releases
            .iter()
            .filter(move |r| r.stage_id.as_deref() == Some(stage_id))
    }

    fn live_plan_stages(&self) -> Vec<(&str, &str, &StageState, &StageConfig)> {
        self.stages
            .iter()
            .filter_map(|(stage_id, def)| match &def.config {
                StageConfig::Plan { environment, .. } => {
                    let state = self.stage_states.get(stage_id.as_str())?;
                    Some((stage_id.as_str(), environment.as_str(), state, &def.config))
                }
                _ => None,
            })
            .filter(|(stage_id, _, state, _)| {
                let planned = self
                    .stage_releases(stage_id)
                    .any(|r| r.mode == "plan" && r.status == "SUCCEEDED");
                planned
                    && matches!(state.status, StageStatus::Active | StageStatus::Succeeded)
                    && state.approval_status != Some(ApprovalStatus::Rejected)
                    && state.stale_by.is_none()
            })
            .collect()
    }

    fn will_apply_in(&self, environment: &str) -> bool {
        self.stages.iter().any(|(stage_id, def)| {
            let StageConfig::Deploy {
                environment: deploy_env,
            } = &def.config
            else {
                return false;
            };
            if deploy_env != environment {
                return false;
            }
            match self.stage_states.get(stage_id.as_str()).map(|s| s.status) {
                None | Some(StageStatus::Pending) => true,
                Some(StageStatus::Active) => self
                    .stage_releases(stage_id)
                    .any(|r| r.mode != "plan" && !is_terminal_release(&r.status)),
                Some(_) => false,
            }
        })
    }

    fn holds_saved_plan(&self) -> bool {
        self.has_saved_plan
            && self.saved_plan_changes
            && self
                .live_plan_stages()
                .into_iter()
                .any(|(_, env, state, config)| {
                    is_approved(config, state) && self.will_apply_in(env)
                })
    }

    fn plans_ahead_of(&self, own: &WaitingPlan) -> bool {
        self.releases.iter().any(|r| {
            r.mode == "plan"
                && !is_terminal_release(&r.status)
                && (r.queued_at, r.release_id) < (own.queued_at, own.release_id)
        })
    }

    fn competes_with(&self, own: &WaitingPlan) -> bool {
        self.intent_id != own.intent_id
            && self.project_id == own.project_id
            && self.intent_status == "ACTIVE"
    }

    pub fn holds(&self, own: &WaitingPlan) -> Option<Hold> {
        if !self.competes_with(own) {
            return None;
        }
        if self.holds_saved_plan() {
            Some(Hold::SavedPlan)
        } else if self.plans_ahead_of(own) {
            Some(Hold::PlanningAhead)
        } else {
            None
        }
    }

    pub fn unapproved_plans_before(&self, own: &WaitingPlan) -> Vec<String> {
        if !self.competes_with(own) || !self.has_saved_plan {
            return Vec::new();
        }
        self.live_plan_stages()
            .into_iter()
            .filter(|(stage_id, env, state, config)| {
                state.status == StageStatus::Active
                    && !is_approved(config, state)
                    && self.will_apply_in(env)
                    && self.stage_releases(stage_id).any(|r| {
                        r.mode == "plan"
                            && (r.queued_at, r.release_id) < (own.queued_at, own.release_id)
                    })
            })
            .map(|(stage_id, ..)| stage_id.to_string())
            .collect()
    }
}

pub fn decide<'a>(own: &WaitingPlan, candidates: &'a [PlanHolder]) -> Decision<'a> {
    if let Some((holder, hold)) = candidates
        .iter()
        .find_map(|c| c.holds(own).map(|hold| (c, hold)))
    {
        return Decision::Wait(holder, hold);
    }
    let retire = candidates
        .iter()
        .map(|c| (c, c.unapproved_plans_before(own)))
        .filter(|(_, stages)| !stages.is_empty())
        .collect();
    Decision::Proceed { retire }
}

pub fn outstanding_holder<'a>(
    own: &WaitingPlan,
    candidates: &'a [PlanHolder],
) -> Option<&'a PlanHolder> {
    match decide(own, candidates) {
        Decision::Wait(holder, _) => Some(holder),
        Decision::Proceed { .. } => None,
    }
}

pub fn retirement(
    policy: Option<&(String, SupersedePendingConfig)>,
    own_branch: Option<&str>,
    holder_branch: Option<&str>,
) -> Retirement {
    let Some((name, config)) = policy else {
        return Retirement::Stale;
    };
    let same_group = !config.same_branch_only
        || matches!((own_branch, holder_branch), (Some(a), Some(b)) if a == b);
    if same_group {
        Retirement::Superseded {
            policy: name.clone(),
        }
    } else {
        Retirement::Stale
    }
}

pub fn waiting_line(slug: &str, destination: &str) -> String {
    format!(
        "forest: {}",
        wait_reason_text(slug, destination, Hold::SavedPlan)
    )
}

pub fn wait_reason_text(slug: &str, destination: &str, hold: Hold) -> String {
    match hold {
        Hold::SavedPlan => {
            format!(
                "waiting for release {slug} to apply its plan for {destination} before planning"
            )
        }
        Hold::PlanningAhead => {
            format!("waiting for release {slug} to finish planning {destination} before planning")
        }
    }
}

pub fn superseded_reason(slug: &str, destination: &str, policy: &str) -> String {
    format!(
        "superseded by release {slug}, which planned {destination} after it \
         (supersede-pending policy '{policy}')"
    )
}

pub fn stale_reason(slug: &str, destination: &str) -> String {
    format!(
        "stale: release {slug} planned {destination} after this plan; reject it and release again to plan afresh"
    )
}

pub async fn load_candidates(
    db: &PgPool,
    own: &WaitingPlan,
    destination_id: Uuid,
) -> anyhow::Result<Vec<PlanHolder>> {
    let intents = sqlx::query!(
        r#"SELECT ri.id, ri.project_id, ri.status, ri.stages, ri.stage_states,
                  a.slug as "slug?",
                  a.ref->>'commit_branch' as "branch?",
                  EXISTS (
                      SELECT 1 FROM terraform_plans tp
                      WHERE tp.release_intent_id = ri.id AND tp.destination_id = $3
                        AND tp.created_at >= now() - make_interval(days => $4)
                  )
                  OR EXISTS (
                      SELECT 1 FROM release_states sealed
                      WHERE sealed.release_intent_id = ri.id AND sealed.destination_id = $3
                        AND sealed.mode = 'plan' AND sealed.status = 'SUCCEEDED'
                        AND sealed.plan_output IS NOT NULL
                        AND sealed.queued_at >= now() - make_interval(days => $4)
                  ) as "has_saved_plan!",
                  EXISTS (
                      SELECT 1 FROM terraform_plans tp
                      WHERE tp.release_intent_id = ri.id AND tp.destination_id = $3
                        AND tp.created_at >= now() - make_interval(days => $4)
                        AND tp.has_changes IS DISTINCT FROM false
                  )
                  OR EXISTS (
                      SELECT 1 FROM release_states sealed
                      WHERE sealed.release_intent_id = ri.id AND sealed.destination_id = $3
                        AND sealed.mode = 'plan' AND sealed.status = 'SUCCEEDED'
                        AND sealed.plan_output IS NOT NULL
                        AND sealed.queued_at >= now() - make_interval(days => $4)
                  ) as "saved_plan_changes!"
           FROM release_intents ri
           LEFT JOIN annotations a ON a.id = ri.annotation_id
           WHERE ri.project_id = $1
             AND ri.id <> $2
             AND ri.status = 'ACTIVE'
             AND ri.stages IS NOT NULL
             AND (
                 EXISTS (
                     SELECT 1 FROM terraform_plans tp
                     WHERE tp.release_intent_id = ri.id AND tp.destination_id = $3
                       AND tp.created_at >= now() - make_interval(days => $4)
                 )
                 OR EXISTS (
                     SELECT 1 FROM release_states sealed
                     WHERE sealed.release_intent_id = ri.id AND sealed.destination_id = $3
                       AND sealed.mode = 'plan' AND sealed.status = 'SUCCEEDED'
                       AND sealed.plan_output IS NOT NULL
                       AND sealed.queued_at >= now() - make_interval(days => $4)
                 )
                 OR EXISTS (
                     SELECT 1 FROM release_states rs
                     WHERE rs.release_intent_id = ri.id AND rs.destination_id = $3
                       AND rs.mode = 'plan' AND rs.status IN ('QUEUED', 'ASSIGNED', 'RUNNING')
                 )
             )
           ORDER BY ri.created"#,
        own.project_id,
        own.intent_id,
        destination_id,
        PLAN_RETENTION_DAYS,
    )
    .fetch_all(db)
    .await
    .context("load intents holding plans")?;

    if intents.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<Uuid> = intents.iter().map(|i| i.id).collect();
    let releases = sqlx::query!(
        "SELECT release_id, release_intent_id, stage_id, mode, status, queued_at
         FROM release_states
         WHERE release_intent_id = ANY($1) AND destination_id = $2",
        &ids,
        destination_id,
    )
    .fetch_all(db)
    .await
    .context("load releases of intents holding plans")?;

    let mut holders = Vec::with_capacity(intents.len());
    for intent in intents {
        let Some(stages) = intent.stages else {
            continue;
        };
        let stages: PipelineStages = match serde_json::from_value(stages) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(intent_id = %intent.id, "plan guard: unreadable stages: {e}");
                continue;
            }
        };
        let stage_states: StageStates = match intent.stage_states.map(serde_json::from_value) {
            Some(Ok(s)) => s,
            Some(Err(e)) => {
                tracing::warn!(intent_id = %intent.id, "plan guard: unreadable stage states: {e}");
                continue;
            }
            None => StageStates::new(),
        };
        holders.push(PlanHolder {
            intent_id: intent.id,
            project_id: intent.project_id,
            slug: intent.slug.unwrap_or_else(|| intent.id.to_string()),
            branch: intent.branch,
            intent_status: intent.status,
            has_saved_plan: intent.has_saved_plan,
            saved_plan_changes: intent.saved_plan_changes,
            stages,
            stage_states,
            releases: releases
                .iter()
                .filter(|r| r.release_intent_id == intent.id)
                .map(|r| HolderRelease {
                    release_id: r.release_id,
                    stage_id: r.stage_id.clone(),
                    mode: r.mode.clone(),
                    status: r.status.clone(),
                    queued_at: r.queued_at,
                })
                .collect(),
        });
    }
    Ok(holders)
}

async fn log_once(db: &PgPool, release: &ReleaseState, line: String) -> anyhow::Result<()> {
    let entry = LogLine {
        channel: LogChannel::Stdout,
        line: line.clone(),
        timestamp: std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default(),
    };
    let probe = serde_json::json!([{ "line": line }]);

    let logged = sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM release_logs
               WHERE release_intent_id = $1 AND destination_id = $2 AND log_lines @> $3
           ) as "logged!""#,
        release.release_intent_id,
        release.destination_id,
        probe,
    )
    .fetch_one(db)
    .await
    .context("look for an earlier line")?;
    if logged {
        return Ok(());
    }

    ReleaseLogsRegistry::new(db.clone())
        .insert_log_block(
            Uuid::now_v7(),
            release.release_intent_id,
            release.destination_id,
            &[entry],
            0,
        )
        .await
        .context("write plan guard line")
}

async fn move_to_back_of_sweep(db: &PgPool, release_id: Uuid) -> anyhow::Result<()> {
    sqlx::query!(
        "UPDATE release_states SET updated_at = now()
         WHERE release_id = $1 AND status = 'QUEUED'",
        release_id,
    )
    .execute(db)
    .await
    .context("mark held plan as rechecked")?;
    Ok(())
}

struct Destination {
    name: String,
    environment: String,
}

struct Own {
    plan: WaitingPlan,
    slug: String,
    branch: Option<String>,
}

async fn guarded(
    db: &PgPool,
    release: &ReleaseState,
) -> anyhow::Result<Option<(Destination, Own, Vec<PlanHolder>)>> {
    if release.mode != "plan" {
        return Ok(None);
    }
    let Some(dest) = sqlx::query!(
        "SELECT name, environment, type_organisation, type_name FROM destinations WHERE id = $1",
        release.destination_id
    )
    .fetch_optional(db)
    .await
    .context("load destination")?
    else {
        return Ok(None);
    };
    if !saves_plans(&dest.type_organisation, &dest.type_name) {
        return Ok(None);
    }

    let own = sqlx::query!(
        r#"SELECT a.slug as "slug?", a.ref->>'commit_branch' as "branch?"
           FROM release_intents ri
           LEFT JOIN annotations a ON a.id = ri.annotation_id
           WHERE ri.id = $1"#,
        release.release_intent_id,
    )
    .fetch_optional(db)
    .await
    .context("load the planning release")?;
    let (slug, branch) = own.map(|o| (o.slug, o.branch)).unwrap_or((None, None));

    let plan = WaitingPlan {
        intent_id: release.release_intent_id,
        project_id: release.project_id,
        release_id: release.release_id,
        queued_at: release.queued_at,
    };
    let candidates = load_candidates(db, &plan, release.destination_id).await?;
    Ok(Some((
        Destination {
            name: dest.name,
            environment: dest.environment,
        },
        Own {
            plan,
            slug: slug.unwrap_or_else(|| release.release_intent_id.to_string()),
            branch,
        },
        candidates,
    )))
}

/// Read-only, unlike `waits_for_outstanding_plan`: it never retires a plan.
pub async fn wait_reason(db: &PgPool, release: &ReleaseState) -> Option<String> {
    let (dest, own, candidates) = match guarded(db, release).await {
        Ok(Some(found)) => found,
        Ok(None) => return None,
        Err(e) => {
            tracing::debug!(release_id = %release.release_id, "plan wait reason unavailable: {e:#}");
            return None;
        }
    };
    match decide(&own.plan, &candidates) {
        Decision::Wait(holder, hold) => Some(wait_reason_text(&holder.slug, &dest.name, hold)),
        Decision::Proceed { .. } => None,
    }
}

enum Retired {
    Done(Option<Retirement>),
    NowApproved,
}

#[allow(clippy::too_many_arguments)]
async fn retire(
    db: &PgPool,
    destination_id: Uuid,
    dest: &Destination,
    own: &Own,
    holder: &PlanHolder,
    stage_ids: &[String],
    kind: &Retirement,
) -> anyhow::Result<Retired> {
    let mut tx = db.begin().await.context("begin retire plan")?;
    let Some(intent) = sqlx::query!(
        "SELECT status, stages, stage_states FROM release_intents WHERE id = $1 FOR UPDATE",
        holder.intent_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .context("lock the older release")?
    else {
        return Ok(Retired::Done(None));
    };
    if intent.status != "ACTIVE" {
        return Ok(Retired::Done(None));
    }
    let stages: PipelineStages = serde_json::from_value(intent.stages.unwrap_or_default())
        .context("parse the older release's stages")?;
    let mut stage_states: StageStates = intent
        .stage_states
        .map(serde_json::from_value)
        .transpose()
        .context("parse the older release's stage states")?
        .unwrap_or_default();

    let now = Utc::now().to_rfc3339();
    let mut changed = false;
    for stage_id in stage_ids {
        let (Some(def), Some(state)) = (stages.get(stage_id), stage_states.get_mut(stage_id))
        else {
            continue;
        };
        if state.status != StageStatus::Active || state.stale_by.is_some() {
            continue;
        }
        if is_approved(&def.config, state) {
            return Ok(Retired::NowApproved);
        }
        match kind {
            Retirement::Superseded { policy } => {
                state.status = StageStatus::Superseded;
                state.approval_status = None;
                state.completed_at = Some(now.clone());
                state.error_message = Some(superseded_reason(&own.slug, &dest.name, policy));
            }
            Retirement::Stale => {
                state.approval_status = Some(ApprovalStatus::AwaitingApproval);
                state.stale_by = Some(own.plan.intent_id.to_string());
                state.error_message = Some(stale_reason(&own.slug, &dest.name));
            }
        }
        changed = true;
    }
    if !changed {
        return Ok(Retired::Done(None));
    }

    sqlx::query!(
        "UPDATE release_intents SET stage_states = $2 WHERE id = $1",
        holder.intent_id,
        serde_json::to_value(&stage_states).context("serialize stage states")?,
    )
    .execute(&mut *tx)
    .await
    .context("retire the older plan")?;
    sqlx::query!(
        "DELETE FROM terraform_plans WHERE release_intent_id = $1 AND destination_id = $2",
        holder.intent_id,
        destination_id,
    )
    .execute(&mut *tx)
    .await
    .context("discard the older plan")?;
    tx.commit().await.context("commit retire plan")?;
    Ok(Retired::Done(Some(kind.clone())))
}

fn retired_line(holder: &str, destination: &str, kind: &Retirement) -> String {
    match kind {
        Retirement::Superseded { policy } => format!(
            "forest: superseded release {holder}'s unapproved plan for {destination} \
             (supersede-pending policy '{policy}')"
        ),
        Retirement::Stale => format!(
            "forest: marked release {holder}'s unapproved plan for {destination} stale; \
             only the newest plan can be approved"
        ),
    }
}

pub async fn waits_for_outstanding_plan(
    db: &PgPool,
    nats: &async_nats::Client,
    policies: &PolicyRegistry,
    release: &ReleaseState,
) -> bool {
    match check(db, nats, policies, release).await {
        Ok(waits) => waits,
        Err(e) => {
            tracing::warn!(
                release_id = %release.release_id,
                "plan guard check failed, planning as normal: {e:#}"
            );
            false
        }
    }
}

async fn check(
    db: &PgPool,
    nats: &async_nats::Client,
    policies: &PolicyRegistry,
    release: &ReleaseState,
) -> anyhow::Result<bool> {
    let Some((dest, own, candidates)) = guarded(db, release).await? else {
        return Ok(false);
    };

    let retire_list = match decide(&own.plan, &candidates) {
        Decision::Wait(holder, hold) => {
            defer(db, release, &dest, holder, hold).await;
            return Ok(true);
        }
        Decision::Proceed { retire } => retire,
    };
    if retire_list.is_empty() {
        return Ok(false);
    }

    let policy = match policies
        .supersede_pending_for_environment(&release.project_id, &dest.environment)
        .await
    {
        Ok(policy) => policy,
        Err(e) => {
            tracing::warn!(release_id = %release.release_id, "plan guard: supersede_pending lookup failed, waiting: {e:#}");
            defer(db, release, &dest, retire_list[0].0, Hold::SavedPlan).await;
            return Ok(true);
        }
    };
    for (holder, stage_ids) in retire_list {
        let kind = retirement(
            policy.as_ref(),
            own.branch.as_deref(),
            holder.branch.as_deref(),
        );
        match retire(
            db,
            release.destination_id,
            &dest,
            &own,
            holder,
            &stage_ids,
            &kind,
        )
        .await
        {
            Err(e) => {
                tracing::warn!(release_id = %release.release_id, "plan guard: could not retire an older plan, waiting: {e:#}");
                defer(db, release, &dest, holder, Hold::SavedPlan).await;
                return Ok(true);
            }
            Ok(Retired::NowApproved) => {
                defer(db, release, &dest, holder, Hold::SavedPlan).await;
                return Ok(true);
            }
            Ok(Retired::Done(None)) => {}
            Ok(Retired::Done(Some(kind))) => {
                tracing::info!(
                    release_id = %release.release_id,
                    older_intent_id = %holder.intent_id,
                    ?kind,
                    "plan guard: retired an older unapproved plan for this destination"
                );
                let _ = nats
                    .publish(
                        "forest.intent.evaluate",
                        holder.intent_id.to_string().into(),
                    )
                    .await;
                if let Err(e) =
                    log_once(db, release, retired_line(&holder.slug, &dest.name, &kind)).await
                {
                    tracing::warn!(release_id = %release.release_id, "could not log retired plan: {e:#}");
                }
            }
        }
    }
    Ok(false)
}

async fn defer(
    db: &PgPool,
    release: &ReleaseState,
    dest: &Destination,
    holder: &PlanHolder,
    hold: Hold,
) {
    tracing::info!(
        release_id = %release.release_id,
        holder_intent_id = %holder.intent_id,
        ?hold,
        "plan deferred: another release holds an unapplied plan for this destination"
    );
    if let Err(e) = move_to_back_of_sweep(db, release.release_id).await {
        tracing::warn!(release_id = %release.release_id, "could not requeue held plan: {e:#}");
    }
    let line = format!(
        "forest: {}",
        wait_reason_text(&holder.slug, &dest.name, hold)
    );
    if let Err(e) = log_once(db, release, line).await {
        tracing::warn!(release_id = %release.release_id, "could not log plan wait: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::release_pipeline::{StageDefinition, StageState};

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap()
    }

    fn agent_gateway_stages() -> PipelineStages {
        let mut stages = PipelineStages::new();
        stages.insert(
            "deploy-dev".into(),
            StageDefinition {
                depends_on: vec![],
                config: StageConfig::Deploy {
                    environment: "dev".into(),
                },
            },
        );
        stages.insert(
            "plan-prod".into(),
            StageDefinition {
                depends_on: vec![],
                config: StageConfig::Plan {
                    environment: "prod".into(),
                    auto_approve: false,
                    destroy: false,
                },
            },
        );
        stages.insert(
            "deploy-prod".into(),
            StageDefinition {
                depends_on: vec!["plan-prod".into(), "deploy-dev".into()],
                config: StageConfig::Deploy {
                    environment: "prod".into(),
                },
            },
        );
        stages
    }

    fn state(status: StageStatus) -> StageState {
        StageState {
            status,
            ..StageState::pending()
        }
    }

    fn release_at(
        stage: &str,
        mode: &str,
        status: &str,
        queued_at: DateTime<Utc>,
    ) -> HolderRelease {
        HolderRelease {
            release_id: Uuid::now_v7(),
            stage_id: Some(stage.into()),
            mode: mode.into(),
            status: status.into(),
            queued_at,
        }
    }

    fn release(stage: &str, mode: &str, status: &str) -> HolderRelease {
        release_at(stage, mode, status, at(0))
    }

    fn approved_plan() -> StageState {
        StageState {
            approval_status: Some(ApprovalStatus::Approved),
            ..state(StageStatus::Succeeded)
        }
    }

    fn holder(project_id: Uuid, deploy_prod: StageState) -> PlanHolder {
        let mut stage_states = StageStates::new();
        stage_states.insert("deploy-dev".into(), state(StageStatus::Active));
        stage_states.insert("plan-prod".into(), approved_plan());
        stage_states.insert("deploy-prod".into(), deploy_prod);
        PlanHolder {
            intent_id: Uuid::now_v7(),
            project_id,
            slug: "brave-otter".into(),
            branch: Some("main".into()),
            intent_status: "ACTIVE".into(),
            has_saved_plan: true,
            saved_plan_changes: true,
            stages: agent_gateway_stages(),
            stage_states,
            releases: vec![release("plan-prod", "plan", "SUCCEEDED")],
        }
    }

    fn in_flight(project_id: Uuid, plan_status: &str, queued_at: DateTime<Utc>) -> PlanHolder {
        let mut a = holder(project_id, state(StageStatus::Pending));
        a.has_saved_plan = false;
        a.stage_states
            .insert("plan-prod".into(), state(StageStatus::Active));
        a.releases = vec![release_at("plan-prod", "plan", plan_status, queued_at)];
        a
    }

    fn me(project_id: Uuid) -> WaitingPlan {
        WaitingPlan {
            intent_id: Uuid::now_v7(),
            project_id,
            release_id: Uuid::now_v7(),
            queued_at: at(100),
        }
    }

    fn waits(own: &WaitingPlan, holders: &[PlanHolder]) -> bool {
        outstanding_holder(own, holders).is_some()
    }

    #[test]
    fn a_plan_waits_in_a_burst_while_an_earlier_release_has_not_applied_its_plan() {
        let project = Uuid::now_v7();
        let holders = [holder(project, state(StageStatus::Pending))];
        let found = outstanding_holder(&me(project), &holders).expect("must wait");
        assert_eq!(found.slug, "brave-otter");
    }

    #[test]
    fn a_plan_waits_while_the_earlier_deploy_is_queued_or_running() {
        let project = Uuid::now_v7();
        for status in ["QUEUED", "ASSIGNED", "RUNNING"] {
            let mut a = holder(project, state(StageStatus::Active));
            a.releases.push(release("deploy-prod", "deploy", status));
            assert!(
                waits(&me(project), &[a]),
                "deploy {status} still has to apply its plan"
            );
        }
    }

    fn awaiting(project_id: Uuid, approval_status: Option<ApprovalStatus>) -> PlanHolder {
        let mut a = holder(project_id, state(StageStatus::Pending));
        a.stage_states.insert(
            "plan-prod".into(),
            StageState {
                approval_status,
                ..state(StageStatus::Active)
            },
        );
        a
    }

    fn retired(own: &WaitingPlan, holders: &[PlanHolder]) -> Vec<(Uuid, Vec<String>)> {
        match decide(own, holders) {
            Decision::Wait(h, _) => panic!("expected to proceed, but {} holds", h.slug),
            Decision::Proceed { retire } => retire
                .into_iter()
                .map(|(h, stages)| (h.intent_id, stages))
                .collect(),
        }
    }

    #[test]
    fn an_earlier_plan_nobody_approved_is_retired_instead_of_waited_for() {
        let project = Uuid::now_v7();
        for approval in [Some(ApprovalStatus::AwaitingApproval), None] {
            let a = awaiting(project, approval);
            let a_id = a.intent_id;
            assert!(
                !waits(&me(project), std::slice::from_ref(&a)),
                "an unapproved plan ({approval:?}) must not hold a newer plan"
            );
            assert_eq!(
                retired(&me(project), &[a]),
                vec![(a_id, vec!["plan-prod".to_string()])],
                "it is retired so it can no longer be applied over the newer plan"
            );
        }
    }

    #[test]
    fn an_auto_approved_plan_holds_before_the_coordinator_marks_it_approved() {
        let project = Uuid::now_v7();
        let mut a = awaiting(project, None);
        a.stages.get_mut("plan-prod").unwrap().config = StageConfig::Plan {
            environment: "prod".into(),
            auto_approve: true,
            destroy: false,
        };
        assert!(waits(&me(project), &[a]));
    }

    #[test]
    fn a_saved_plan_that_changes_nothing_never_holds() {
        let project = Uuid::now_v7();
        let mut approved = holder(project, state(StageStatus::Pending));
        approved.saved_plan_changes = false;
        assert!(!waits(&me(project), std::slice::from_ref(&approved)));
        assert!(retired(&me(project), &[approved]).is_empty());

        let mut parked = awaiting(project, Some(ApprovalStatus::AwaitingApproval));
        parked.saved_plan_changes = false;
        assert!(!waits(&me(project), std::slice::from_ref(&parked)));
        assert_eq!(
            retired(&me(project), &[parked]).len(),
            1,
            "a parked no-op plan is still retired, so only the newest plan can be approved"
        );
    }

    #[test]
    fn a_stale_plan_neither_holds_nor_is_retired_again() {
        let project = Uuid::now_v7();
        let mut a = awaiting(project, Some(ApprovalStatus::AwaitingApproval));
        a.stage_states.get_mut("plan-prod").unwrap().stale_by = Some(Uuid::now_v7().to_string());
        assert!(!waits(&me(project), std::slice::from_ref(&a)));
        assert!(retired(&me(project), &[a]).is_empty());
    }

    #[test]
    fn a_plan_made_after_this_one_is_never_retired_by_it() {
        let project = Uuid::now_v7();
        let mut a = awaiting(project, Some(ApprovalStatus::AwaitingApproval));
        a.releases = vec![release_at("plan-prod", "plan", "SUCCEEDED", at(200))];
        assert!(retired(&me(project), &[a]).is_empty());
    }

    #[test]
    fn a_plan_whose_release_was_cancelled_or_rejected_is_neither_held_nor_retired() {
        let project = Uuid::now_v7();

        let mut cancelled = awaiting(project, Some(ApprovalStatus::AwaitingApproval));
        cancelled.intent_status = "CANCELLED".into();
        let mut deploy_cancelled = awaiting(project, Some(ApprovalStatus::AwaitingApproval));
        deploy_cancelled
            .stage_states
            .insert("deploy-prod".into(), state(StageStatus::Cancelled));
        let rejected = awaiting(project, Some(ApprovalStatus::Rejected));
        let mut superseded = awaiting(project, None);
        superseded
            .stage_states
            .insert("plan-prod".into(), state(StageStatus::Superseded));

        for h in [cancelled, deploy_cancelled, rejected, superseded] {
            assert!(!waits(&me(project), std::slice::from_ref(&h)));
            assert!(retired(&me(project), &[h]).is_empty());
        }
    }

    #[test]
    fn a_holder_that_must_be_waited_for_wins_over_retiring_others() {
        let project = Uuid::now_v7();
        let parked = awaiting(project, Some(ApprovalStatus::AwaitingApproval));
        let approved = holder(project, state(StageStatus::Pending));
        let approved_id = approved.intent_id;
        match decide(&me(project), &[parked, approved]) {
            Decision::Wait(h, Hold::SavedPlan) => assert_eq!(h.intent_id, approved_id),
            other => panic!("expected to wait for the approved plan, got {other:?}"),
        }
    }

    fn supersede_pending(same_branch_only: bool) -> (String, SupersedePendingConfig) {
        (
            "collapse-prod".into(),
            SupersedePendingConfig {
                target_environment: "prod".into(),
                same_branch_only,
                cancel_in_progress: false,
            },
        )
    }

    #[test]
    fn without_a_supersede_pending_policy_an_older_plan_is_marked_stale() {
        assert_eq!(
            retirement(None, Some("main"), Some("main")),
            Retirement::Stale
        );
    }

    #[test]
    fn a_supersede_pending_policy_supersedes_the_older_plan() {
        let superseded = Retirement::Superseded {
            policy: "collapse-prod".into(),
        };
        let any_branch = supersede_pending(false);
        assert_eq!(
            retirement(Some(&any_branch), Some("main"), Some("renovate/x")),
            superseded
        );

        let same_branch = supersede_pending(true);
        assert_eq!(
            retirement(Some(&same_branch), Some("main"), Some("main")),
            superseded
        );
        assert_eq!(
            retirement(Some(&same_branch), Some("main"), Some("renovate/x")),
            Retirement::Stale,
            "another branch is outside the policy's group, so it is only marked stale"
        );
        assert_eq!(
            retirement(Some(&same_branch), Some("main"), None),
            Retirement::Stale
        );
    }

    #[test]
    fn a_plan_proceeds_once_the_earlier_deploy_has_finished() {
        let project = Uuid::now_v7();
        for status in [
            "SUCCEEDED",
            "FAILED",
            "CANCELLED",
            "TIMED_OUT",
            "SUPERSEDED",
        ] {
            let mut a = holder(project, state(StageStatus::Active));
            a.releases.push(release("deploy-prod", "deploy", status));
            assert!(
                !waits(&me(project), &[a]),
                "deploy {status} will not apply the plan"
            );
        }
        let a = holder(project, state(StageStatus::Succeeded));
        assert!(!waits(&me(project), &[a]));
    }

    #[test]
    fn a_rejected_plan_does_not_block() {
        let project = Uuid::now_v7();
        let mut awaiting_evaluation = holder(project, state(StageStatus::Pending));
        awaiting_evaluation.stage_states.insert(
            "plan-prod".into(),
            StageState {
                approval_status: Some(ApprovalStatus::Rejected),
                ..state(StageStatus::Active)
            },
        );
        assert!(!waits(&me(project), &[awaiting_evaluation]));

        let mut evaluated = holder(project, state(StageStatus::Cancelled));
        evaluated.stage_states.insert(
            "plan-prod".into(),
            StageState {
                approval_status: Some(ApprovalStatus::Rejected),
                ..state(StageStatus::Failed)
            },
        );
        assert!(!waits(&me(project), &[evaluated]));
    }

    #[test]
    fn a_plan_whose_deploy_was_cancelled_after_dev_failed_does_not_block() {
        let project = Uuid::now_v7();
        let mut a = holder(project, state(StageStatus::Cancelled));
        a.stage_states
            .insert("deploy-dev".into(), state(StageStatus::Failed));
        assert!(!waits(&me(project), std::slice::from_ref(&a)));

        let mut finished = a;
        finished.intent_status = "FAILED".into();
        finished
            .stage_states
            .insert("deploy-prod".into(), state(StageStatus::Pending));
        assert!(!waits(&me(project), &[finished]));
    }

    #[test]
    fn a_superseded_plan_does_not_block() {
        let project = Uuid::now_v7();
        let mut plan_superseded = holder(project, state(StageStatus::Pending));
        plan_superseded
            .stage_states
            .insert("plan-prod".into(), state(StageStatus::Superseded));
        assert!(!waits(&me(project), &[plan_superseded]));

        let deploy_superseded = holder(project, state(StageStatus::Superseded));
        assert!(!waits(&me(project), &[deploy_superseded]));
    }

    #[test]
    fn a_discarded_pruned_or_expired_plan_does_not_block() {
        let project = Uuid::now_v7();
        let mut a = holder(project, state(StageStatus::Pending));
        a.has_saved_plan = false;
        assert!(!waits(&me(project), &[a]));
    }

    #[test]
    fn an_earlier_plan_still_in_flight_holds_a_later_one() {
        let project = Uuid::now_v7();
        for status in ["QUEUED", "ASSIGNED", "RUNNING"] {
            let a = in_flight(project, status, at(0));
            assert!(
                waits(&me(project), &[a]),
                "an earlier plan {status} will apply first, so planning now would go stale"
            );
        }
    }

    #[test]
    fn a_later_plan_in_flight_never_holds_an_earlier_one() {
        let project = Uuid::now_v7();
        let later = in_flight(project, "QUEUED", at(200));
        assert!(!waits(&me(project), &[later]));
    }

    #[test]
    fn two_plans_queued_at_the_same_instant_never_hold_each_other() {
        let project = Uuid::now_v7();
        let mut a = in_flight(project, "QUEUED", at(100));
        let mut b = in_flight(project, "QUEUED", at(100));
        let a_plan = a.releases[0].release_id;
        let b_plan = b.releases[0].release_id;
        a.releases[0].release_id = a_plan.min(b_plan);
        b.releases[0].release_id = a_plan.max(b_plan);

        let a_waiting = WaitingPlan {
            intent_id: a.intent_id,
            project_id: project,
            release_id: a.releases[0].release_id,
            queued_at: at(100),
        };
        let b_waiting = WaitingPlan {
            intent_id: b.intent_id,
            project_id: project,
            release_id: b.releases[0].release_id,
            queued_at: at(100),
        };
        let both = [a, b];
        assert!(!waits(&a_waiting, &both), "the lower release id goes first");
        assert!(waits(&b_waiting, &both), "the higher release id waits");
    }

    #[test]
    fn an_earlier_plan_that_failed_or_was_superseded_does_not_hold() {
        let project = Uuid::now_v7();
        for status in ["FAILED", "CANCELLED", "TIMED_OUT", "SUPERSEDED"] {
            let mut a = in_flight(project, status, at(0));
            a.stage_states
                .insert("plan-prod".into(), state(StageStatus::Failed));
            assert!(!waits(&me(project), &[a]), "plan {status} never applies");
        }
    }

    #[test]
    fn the_releases_own_intent_never_blocks_it() {
        let project = Uuid::now_v7();
        let a = holder(project, state(StageStatus::Pending));
        let own = WaitingPlan {
            intent_id: a.intent_id,
            ..me(project)
        };
        assert!(!waits(&own, &[a]));
    }

    #[test]
    fn another_projects_plan_never_blocks() {
        let saved = holder(Uuid::now_v7(), state(StageStatus::Pending));
        let running = in_flight(Uuid::now_v7(), "RUNNING", at(0));
        assert!(!waits(&me(Uuid::now_v7()), &[saved, running]));
    }

    #[test]
    fn a_plan_for_another_environment_does_not_block() {
        let project = Uuid::now_v7();
        let mut a = holder(project, state(StageStatus::Pending));
        a.stages.remove("deploy-prod");
        assert!(!waits(&me(project), &[a]));
    }

    #[test]
    fn terraform_and_generic_destinations_save_plans() {
        assert!(saves_plans("forest", "terraform"));
        assert!(saves_plans("forest", "generic"));
        assert!(!saves_plans("forest", "kubernetes"));
        assert!(!saves_plans("acme", "terraform"));
    }

    #[test]
    fn the_waiting_line_names_the_release_and_destination() {
        assert_eq!(
            waiting_line("brave-otter", "prod-eu"),
            "forest: waiting for release brave-otter to apply its plan for prod-eu before planning"
        );
        assert_eq!(
            wait_reason_text("brave-otter", "prod-eu", Hold::PlanningAhead),
            "waiting for release brave-otter to finish planning prod-eu before planning"
        );
    }

    #[test]
    fn a_retired_plan_says_which_release_made_it_so() {
        assert_eq!(
            stale_reason("calm-heron", "prod-eu"),
            "stale: release calm-heron planned prod-eu after this plan; reject it and release again to plan afresh"
        );
        assert_eq!(
            superseded_reason("calm-heron", "prod-eu", "collapse-prod"),
            "superseded by release calm-heron, which planned prod-eu after it \
             (supersede-pending policy 'collapse-prod')"
        );
    }
}
