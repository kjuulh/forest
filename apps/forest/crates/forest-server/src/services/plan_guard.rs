use std::time::UNIX_EPOCH;

use anyhow::Context;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::destinations::terraform_plans::PLAN_RETENTION_DAYS;
use crate::services::release_event_store::ReleaseState;
use crate::services::release_logs_registry::{LogChannel, LogLine, ReleaseLogsRegistry};
use crate::services::release_pipeline::{
    ApprovalStatus, PipelineStages, StageConfig, StageStates, StageStatus,
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
    pub intent_status: String,
    pub has_saved_plan: bool,
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

fn is_terminal_release(status: &str) -> bool {
    matches!(
        status,
        "SUCCEEDED" | "FAILED" | "CANCELLED" | "TIMED_OUT" | "SUPERSEDED"
    )
}

pub fn saves_plans(type_organisation: &str, type_name: &str) -> bool {
    type_organisation == "forest" && type_name == "terraform"
}

impl PlanHolder {
    fn stage_releases<'a>(&'a self, stage_id: &'a str) -> impl Iterator<Item = &'a HolderRelease> {
        self.releases
            .iter()
            .filter(move |r| r.stage_id.as_deref() == Some(stage_id))
    }

    fn live_plan_environments(&self) -> Vec<&str> {
        self.stages
            .iter()
            .filter_map(|(stage_id, def)| match &def.config {
                StageConfig::Plan { environment, .. } => Some((stage_id, environment)),
                _ => None,
            })
            .filter(|(stage_id, _)| {
                let planned = self
                    .stage_releases(stage_id)
                    .any(|r| r.mode == "plan" && r.status == "SUCCEEDED");
                let Some(state) = self.stage_states.get(stage_id.as_str()) else {
                    return false;
                };
                planned
                    && matches!(state.status, StageStatus::Active | StageStatus::Succeeded)
                    && state.approval_status != Some(ApprovalStatus::Rejected)
            })
            .map(|(_, environment)| environment.as_str())
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
            && self
                .live_plan_environments()
                .into_iter()
                .any(|env| self.will_apply_in(env))
    }

    fn plans_ahead_of(&self, own: &WaitingPlan) -> bool {
        self.releases.iter().any(|r| {
            r.mode == "plan"
                && !is_terminal_release(&r.status)
                && (r.queued_at, r.release_id) < (own.queued_at, own.release_id)
        })
    }

    pub fn holds(&self, own: &WaitingPlan) -> bool {
        self.intent_id != own.intent_id
            && self.project_id == own.project_id
            && self.intent_status == "ACTIVE"
            && (self.holds_saved_plan() || self.plans_ahead_of(own))
    }
}

pub fn outstanding_holder<'a>(
    own: &WaitingPlan,
    candidates: &'a [PlanHolder],
) -> Option<&'a PlanHolder> {
    candidates.iter().find(|c| c.holds(own))
}

pub fn waiting_line(slug: &str, destination: &str) -> String {
    format!(
        "forest: waiting for release {slug} to apply its plan for {destination} before planning"
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
                  EXISTS (
                      SELECT 1 FROM terraform_plans tp
                      WHERE tp.release_intent_id = ri.id AND tp.destination_id = $3
                        AND tp.created_at >= now() - make_interval(days => $4)
                  ) as "has_saved_plan!"
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
            intent_status: intent.status,
            has_saved_plan: intent.has_saved_plan,
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

async fn log_waiting_once(
    db: &PgPool,
    release: &ReleaseState,
    destination: &str,
    holder: &PlanHolder,
) -> anyhow::Result<()> {
    let line = waiting_line(&holder.slug, destination);
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
    .context("look for an earlier waiting line")?;
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
        .context("write waiting line")
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

async fn holder_for(
    db: &PgPool,
    release: &ReleaseState,
) -> anyhow::Result<Option<(String, PlanHolder)>> {
    let Some(dest) = sqlx::query!(
        "SELECT name, type_organisation, type_name FROM destinations WHERE id = $1",
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

    let own = WaitingPlan {
        intent_id: release.release_intent_id,
        project_id: release.project_id,
        release_id: release.release_id,
        queued_at: release.queued_at,
    };
    let candidates = load_candidates(db, &own, release.destination_id).await?;
    Ok(outstanding_holder(&own, &candidates).map(|h| (dest.name, h.clone())))
}

pub async fn waits_for_outstanding_plan(db: &PgPool, release: &ReleaseState) -> bool {
    if release.mode != "plan" {
        return false;
    }

    let (destination, holder) = match holder_for(db, release).await {
        Ok(Some(found)) => found,
        Ok(None) => return false,
        Err(e) => {
            tracing::warn!(
                release_id = %release.release_id,
                "plan guard check failed, planning as normal: {e:#}"
            );
            return false;
        }
    };

    tracing::info!(
        release_id = %release.release_id,
        holder_intent_id = %holder.intent_id,
        "plan deferred: another release holds an unapplied plan for this destination"
    );
    if let Err(e) = move_to_back_of_sweep(db, release.release_id).await {
        tracing::warn!(release_id = %release.release_id, "could not requeue held plan: {e:#}");
    }
    if let Err(e) = log_waiting_once(db, release, &destination, &holder).await {
        tracing::warn!(release_id = %release.release_id, "could not log plan wait: {e:#}");
    }
    true
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
            intent_status: "ACTIVE".into(),
            has_saved_plan: true,
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

    #[test]
    fn a_plan_waits_while_the_earlier_plan_awaits_approval() {
        let project = Uuid::now_v7();
        let mut a = holder(project, state(StageStatus::Pending));
        a.stage_states.insert(
            "plan-prod".into(),
            StageState {
                approval_status: Some(ApprovalStatus::AwaitingApproval),
                ..state(StageStatus::Active)
            },
        );
        assert!(waits(&me(project), &[a]));
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
    fn only_terraform_destinations_save_plans() {
        assert!(saves_plans("forest", "terraform"));
        assert!(!saves_plans("forest", "kubernetes"));
        assert!(!saves_plans("acme", "terraform"));
    }

    #[test]
    fn the_waiting_line_names_the_release_and_destination() {
        assert_eq!(
            waiting_line("brave-otter", "prod-eu"),
            "forest: waiting for release brave-otter to apply its plan for prod-eu before planning"
        );
    }
}
