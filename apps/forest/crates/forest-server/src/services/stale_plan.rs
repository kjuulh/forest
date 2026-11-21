use std::collections::HashSet;

use anyhow::Context;
use uuid::Uuid;

use crate::destinations::genericv1::STALE_PLAN_PREFIX;
use crate::services::release_pipeline::{PipelineStages, StageConfig, StageStates, StageStatus};

pub const MAX_REPLANS: usize = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Followup {
    Supersede {
        reason: String,
    },
    Replan {
        plan_stages: Vec<String>,
        note: String,
    },
    Wait,
    Fail,
}

pub fn is_stale(error_message: Option<&str>) -> bool {
    error_message.is_some_and(|e| e.starts_with(STALE_PLAN_PREFIX))
}

pub fn decide(
    destinations: &str,
    newer_slug: Option<&str>,
    stale_attempts: usize,
    replan_scope: Vec<String>,
    plan_shared_in_flight: bool,
) -> Followup {
    if let Some(slug) = newer_slug {
        return Followup::Supersede {
            reason: superseded_reason(slug, destinations),
        };
    }
    if stale_attempts > MAX_REPLANS || replan_scope.is_empty() {
        return Followup::Fail;
    }
    if plan_shared_in_flight {
        return Followup::Wait;
    }
    Followup::Replan {
        plan_stages: replan_scope,
        note: replan_note(destinations),
    }
}

pub fn superseded_reason(slug: &str, destinations: &str) -> String {
    format!(
        "superseded by release {slug}: {destinations} changed after this release planned, \
         and {slug} releases it next"
    )
}

pub fn replan_note(destinations: &str) -> String {
    format!(
        "re-planning: {destinations} changed after the approved plan, so the plan is \
         made again before deploying"
    )
}

fn ancestors<'a>(stages: &'a PipelineStages, stage_id: &'a str) -> HashSet<&'a str> {
    let mut stack: Vec<&str> = stages
        .get(stage_id)
        .map(|s| s.depends_on.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let mut seen = HashSet::new();
    while let Some(id) = stack.pop() {
        if seen.insert(id)
            && let Some(stage) = stages.get(id)
        {
            stack.extend(stage.depends_on.iter().map(String::as_str));
        }
    }
    seen
}

pub fn plan_stages_for(stages: &PipelineStages, deploy_stage: &str) -> Vec<String> {
    let Some(StageConfig::Deploy { environment }) = stages.get(deploy_stage).map(|s| &s.config)
    else {
        return Vec::new();
    };
    let mut found: Vec<String> = ancestors(stages, deploy_stage)
        .into_iter()
        .filter(|id| {
            matches!(
                stages.get(*id).map(|s| &s.config),
                Some(StageConfig::Plan { environment: planned, .. }) if planned == environment
            )
        })
        .map(str::to_owned)
        .collect();
    found.sort();
    found
}

/// The stages a replan resets: the deploy's plan stages and every stage between
/// them and the deploy (a gate or wait after the plan runs again too), so the
/// deploy can't become ready before its fresh plan exists.
pub fn replan_scope(stages: &PipelineStages, deploy_stage: &str) -> Vec<String> {
    let plans = plan_stages_for(stages, deploy_stage);
    let mut scope: Vec<String> = ancestors(stages, deploy_stage)
        .into_iter()
        .filter(|id| {
            !matches!(
                stages.get(*id).map(|s| &s.config),
                Some(StageConfig::Deploy { .. })
            )
        })
        .filter(|id| {
            plans.iter().any(|plan| plan == id) || {
                let above = ancestors(stages, id);
                plans.iter().any(|plan| above.contains(plan.as_str()))
            }
        })
        .map(str::to_owned)
        .collect();
    scope.sort();
    if plans.is_empty() { Vec::new() } else { scope }
}

pub fn plan_shared_in_flight(
    stages: &PipelineStages,
    states: &StageStates,
    deploy_stage: &str,
    scope: &[String],
) -> bool {
    stages.iter().any(|(id, def)| {
        id != deploy_stage
            && matches!(def.config, StageConfig::Deploy { .. })
            && states
                .get(id)
                .is_some_and(|s| s.status == StageStatus::Active)
            && ancestors(stages, id)
                .iter()
                .any(|above| scope.iter().any(|reset| reset == above))
    })
}

pub async fn newer_release(
    tx: &mut sqlx::PgConnection,
    intent_id: Uuid,
    project_id: Uuid,
    destination_ids: &[Uuid],
    same_branch: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let newer = sqlx::query!(
        r#"SELECT ri.id, a.slug as "slug?"
           FROM release_intents ri
           LEFT JOIN annotations a ON a.id = ri.annotation_id
           WHERE ri.project_id = $1
             AND ri.id <> $2
             AND ri.created > (SELECT created FROM release_intents WHERE id = $2)
             AND ($4::text IS NULL OR a.ref->>'commit_branch' = $4)
             AND (
                 (ri.status = 'ACTIVE' AND EXISTS (
                     SELECT 1 FROM release_states rs
                     WHERE rs.release_intent_id = ri.id
                       AND rs.destination_id = ANY($3)
                       AND rs.status NOT IN ('FAILED', 'CANCELLED', 'TIMED_OUT', 'SUPERSEDED')
                 ))
                 OR EXISTS (
                     SELECT 1 FROM release_states rs
                     WHERE rs.release_intent_id = ri.id
                       AND rs.destination_id = ANY($3)
                       AND rs.mode <> 'plan' AND rs.status = 'SUCCEEDED'
                 )
             )
           ORDER BY ri.created DESC
           LIMIT 1"#,
        project_id,
        intent_id,
        destination_ids,
        same_branch,
    )
    .fetch_optional(&mut *tx)
    .await
    .context("look for a newer release of the same destination")?;
    Ok(newer.map(|n| n.slug.unwrap_or_else(|| n.id.to_string())))
}

pub async fn intent_branch(
    tx: &mut sqlx::PgConnection,
    intent_id: Uuid,
) -> anyhow::Result<Option<String>> {
    let branch = sqlx::query_scalar!(
        r#"SELECT a.ref->>'commit_branch' as "branch?"
           FROM release_intents ri
           JOIN annotations a ON a.id = ri.annotation_id
           WHERE ri.id = $1"#,
        intent_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .context("read the release's branch")?;
    Ok(branch.flatten())
}

pub async fn destination_names(
    tx: &mut sqlx::PgConnection,
    destination_ids: &[Uuid],
) -> anyhow::Result<String> {
    let mut names = sqlx::query_scalar!(
        "SELECT name FROM destinations WHERE id = ANY($1)",
        destination_ids,
    )
    .fetch_all(&mut *tx)
    .await
    .context("name the stale destinations")?;
    names.sort();
    Ok(names.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipeline() -> PipelineStages {
        serde_json::from_value(serde_json::json!({
            "plan-dev": { "type": "plan", "environment": "dev", "auto_approve": true },
            "deploy-dev": { "type": "deploy", "environment": "dev", "depends_on": ["plan-dev"] },
            "plan-prod": {
                "type": "plan", "environment": "prod", "auto_approve": true,
                "depends_on": ["deploy-dev"]
            },
            "deploy-prod": { "type": "deploy", "environment": "prod", "depends_on": ["plan-prod"] }
        }))
        .unwrap()
    }

    #[test]
    fn a_deploy_replans_only_the_plan_for_its_own_environment() {
        let stages = pipeline();
        assert_eq!(plan_stages_for(&stages, "deploy-dev"), vec!["plan-dev"]);
        assert_eq!(plan_stages_for(&stages, "deploy-prod"), vec!["plan-prod"]);
        assert!(plan_stages_for(&stages, "plan-dev").is_empty());
    }

    #[test]
    fn a_newer_release_for_the_destination_supersedes_instead_of_failing() {
        let followup = decide(
            "grund-pakkelyst-dev",
            Some("generally-grand-koala"),
            1,
            vec!["plan-dev".into()],
            false,
        );
        assert_eq!(
            followup,
            Followup::Supersede {
                reason: "superseded by release generally-grand-koala: grund-pakkelyst-dev \
                         changed after this release planned, and generally-grand-koala \
                         releases it next"
                    .into()
            }
        );
    }

    #[test]
    fn without_a_newer_release_the_first_stale_deploy_replans() {
        assert_eq!(
            decide(
                "grund-pakkelyst-dev",
                None,
                1,
                vec!["plan-dev".into()],
                false
            ),
            Followup::Replan {
                plan_stages: vec!["plan-dev".into()],
                note: replan_note("grund-pakkelyst-dev"),
            }
        );
    }

    #[test]
    fn a_destination_that_keeps_moving_fails_after_one_replan() {
        assert_eq!(
            decide(
                "grund-pakkelyst-dev",
                None,
                2,
                vec!["plan-dev".into()],
                false
            ),
            Followup::Fail
        );
    }

    #[test]
    fn a_deploy_without_a_plan_stage_cannot_replan() {
        assert_eq!(decide("d", None, 1, Vec::new(), false), Followup::Fail);
    }

    #[test]
    fn a_replan_also_resets_a_gate_between_the_plan_and_the_deploy() {
        let stages: PipelineStages = serde_json::from_value(serde_json::json!({
            "plan-dev": { "type": "plan", "environment": "dev", "auto_approve": true },
            "soak": { "type": "wait", "duration_seconds": 60, "depends_on": ["plan-dev"] },
            "deploy-dev": { "type": "deploy", "environment": "dev", "depends_on": ["soak"] },
            "deploy-other": { "type": "deploy", "environment": "dev", "depends_on": ["plan-dev"] },
            "plan-prod": {
                "type": "plan", "environment": "prod", "auto_approve": true,
                "depends_on": ["deploy-dev"]
            },
            "deploy-prod": { "type": "deploy", "environment": "prod", "depends_on": ["plan-prod"] }
        }))
        .unwrap();
        assert_eq!(
            replan_scope(&stages, "deploy-dev"),
            vec!["plan-dev", "soak"]
        );
        assert_eq!(replan_scope(&stages, "deploy-prod"), vec!["plan-prod"]);

        let scope = replan_scope(&stages, "deploy-dev");
        let mut states: StageStates = serde_json::from_value(serde_json::json!({
            "deploy-other": { "status": "SUCCEEDED" }
        }))
        .unwrap();
        assert!(!plan_shared_in_flight(
            &stages,
            &states,
            "deploy-dev",
            &scope
        ));
        states.get_mut("deploy-other").unwrap().status = StageStatus::Active;
        assert!(
            plan_shared_in_flight(&stages, &states, "deploy-dev", &scope),
            "a sibling still deploying from the same plan must keep it"
        );
    }

    #[test]
    fn a_replan_waits_while_a_sibling_still_deploys_the_same_plan() {
        assert_eq!(
            decide("d", None, 1, vec!["plan-dev".into()], true),
            Followup::Wait
        );
    }

    #[test]
    fn a_stale_prod_deploy_never_resets_a_dev_deploy_that_succeeded() {
        let stages: PipelineStages = serde_json::from_value(serde_json::json!({
            "plan-dev": { "type": "plan", "environment": "dev", "auto_approve": true },
            "plan-prod": { "type": "plan", "environment": "prod", "auto_approve": true },
            "deploy-dev": {
                "type": "deploy", "environment": "dev", "depends_on": ["plan-dev", "plan-prod"]
            },
            "deploy-prod": { "type": "deploy", "environment": "prod", "depends_on": ["deploy-dev"] }
        }))
        .unwrap();
        assert_eq!(replan_scope(&stages, "deploy-prod"), vec!["plan-prod"]);
    }

    #[test]
    fn only_a_provider_classified_stale_plan_counts() {
        assert!(is_stale(Some(&format!(
            "{STALE_PLAN_PREFIX}approved stack version changed; request a new plan"
        ))));
        assert!(!is_stale(Some(
            "provider reported failure: approved stack version changed; request a new plan"
        )));
        assert!(!is_stale(None));
    }
}
