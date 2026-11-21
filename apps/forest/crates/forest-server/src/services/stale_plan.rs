use std::collections::HashSet;

use anyhow::Context;
use uuid::Uuid;

use crate::destinations::genericv1::STALE_PLAN_PREFIX;
use crate::services::release_pipeline::{PipelineStages, StageConfig};

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
    Fail,
}

pub fn is_stale(error_message: Option<&str>) -> bool {
    error_message.is_some_and(|e| e.starts_with(STALE_PLAN_PREFIX))
}

pub fn decide(
    destinations: &str,
    newer_slug: Option<&str>,
    stale_attempts: usize,
    plan_stages: Vec<String>,
) -> Followup {
    if let Some(slug) = newer_slug {
        return Followup::Supersede {
            reason: superseded_reason(slug, destinations),
        };
    }
    if stale_attempts > MAX_REPLANS || plan_stages.is_empty() {
        return Followup::Fail;
    }
    Followup::Replan {
        plan_stages,
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

pub fn plan_stages_for(stages: &PipelineStages, deploy_stage: &str) -> Vec<String> {
    let Some(deploy) = stages.get(deploy_stage) else {
        return Vec::new();
    };
    let StageConfig::Deploy { environment } = &deploy.config else {
        return Vec::new();
    };
    let mut stack: Vec<&str> = deploy.depends_on.iter().map(String::as_str).collect();
    let mut visited = HashSet::new();
    let mut found = Vec::new();
    while let Some(stage_id) = stack.pop() {
        if !visited.insert(stage_id) {
            continue;
        }
        let Some(stage) = stages.get(stage_id) else {
            continue;
        };
        if let StageConfig::Plan {
            environment: planned,
            ..
        } = &stage.config
            && planned == environment
        {
            found.push(stage_id.to_string());
        }
        stack.extend(stage.depends_on.iter().map(String::as_str));
    }
    found.sort();
    found
}

pub async fn newer_release(
    tx: &mut sqlx::PgConnection,
    intent_id: Uuid,
    project_id: Uuid,
    destination_ids: &[Uuid],
) -> anyhow::Result<Option<String>> {
    let newer = sqlx::query!(
        r#"SELECT ri.id, a.slug as "slug?"
           FROM release_intents ri
           LEFT JOIN annotations a ON a.id = ri.annotation_id
           WHERE ri.project_id = $1
             AND ri.id <> $2
             AND ri.created > (SELECT created FROM release_intents WHERE id = $2)
             AND EXISTS (
                 SELECT 1 FROM release_states rs
                 WHERE rs.release_intent_id = ri.id
                   AND rs.destination_id = ANY($3)
                   AND rs.status NOT IN ('FAILED', 'CANCELLED', 'TIMED_OUT', 'SUPERSEDED')
             )
           ORDER BY ri.created DESC
           LIMIT 1"#,
        project_id,
        intent_id,
        destination_ids,
    )
    .fetch_optional(&mut *tx)
    .await
    .context("look for a newer release of the same destination")?;
    Ok(newer.map(|n| n.slug.unwrap_or_else(|| n.id.to_string())))
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
            decide("grund-pakkelyst-dev", None, 1, vec!["plan-dev".into()]),
            Followup::Replan {
                plan_stages: vec!["plan-dev".into()],
                note: replan_note("grund-pakkelyst-dev"),
            }
        );
    }

    #[test]
    fn a_destination_that_keeps_moving_fails_after_one_replan() {
        assert_eq!(
            decide("grund-pakkelyst-dev", None, 2, vec!["plan-dev".into()]),
            Followup::Fail
        );
    }

    #[test]
    fn a_deploy_without_a_plan_stage_cannot_replan() {
        assert_eq!(decide("d", None, 1, Vec::new()), Followup::Fail);
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
