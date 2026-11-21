use forest_server::destinations::genericv1::STALE_PLAN_PREFIX;
use forest_server::intent_coordinator;
use forest_server::services::plan_guard;
use forest_server::services::policy::PolicyRegistryState;
use forest_server::services::release_event_store::ReleaseEventStoreState;
use uuid::Uuid;

use crate::accepttest::fixtures::{Fixture, fixture};

const STALE: &str = "approved stack version changed; request a new plan";

struct Scene {
    f: Fixture,
    project_id: Uuid,
    destination_id: Uuid,
}

fn pipeline() -> serde_json::Value {
    serde_json::json!({
        "plan-dev": { "type": "plan", "environment": "dev", "auto_approve": true, "depends_on": [] },
        "deploy-dev": { "type": "deploy", "environment": "dev", "depends_on": ["plan-dev"] }
    })
}

impl Scene {
    async fn new() -> anyhow::Result<Self> {
        let f = fixture().await?;
        let suffix = Uuid::now_v7();
        let org = format!("grund-replay-{suffix}");
        sqlx::query!(
            "INSERT INTO organisations (id, name) VALUES ($1, $2)",
            Uuid::now_v7(),
            org,
        )
        .execute(&f.db)
        .await?;
        let environment_id = sqlx::query_scalar!(
            "INSERT INTO environments (organisation, name) VALUES ($1, 'dev') RETURNING id",
            org,
        )
        .fetch_one(&f.db)
        .await?;
        let destination_id = sqlx::query_scalar!(
            r#"INSERT INTO destinations
                   (name, environment, organisation, type_organisation, type_name,
                    type_version, metadata, environment_id)
               VALUES ($1, 'dev', $2, 'forest', 'generic', 1, '{}', $3)
               RETURNING id"#,
            format!("grund-pakkelyst-dev-{suffix}"),
            org,
            environment_id,
        )
        .fetch_one(&f.db)
        .await?;
        let project_id = sqlx::query_scalar!(
            "INSERT INTO projects (organisation, project) VALUES ($1, 'pakkelyst') RETURNING id",
            org,
        )
        .fetch_one(&f.db)
        .await?;
        Ok(Self {
            f,
            project_id,
            destination_id,
        })
    }

    async fn intent(&self, slug: &str) -> Uuid {
        let id = Uuid::now_v7();
        let annotation = Uuid::now_v7();
        let artifact = Uuid::now_v7();
        sqlx::query!(
            r#"INSERT INTO release_intents
                   (id, artifact, annotation_id, project_id, status, stages, stage_states)
               VALUES ($1, $2, $3, $4, 'ACTIVE', $5, '{}')"#,
            id,
            artifact,
            annotation,
            self.project_id,
            pipeline(),
        )
        .execute(&self.f.db)
        .await
        .expect("insert intent");
        sqlx::query!(
            r#"INSERT INTO annotations
                   (id, slug, artifact_id, metadata, source, context, project_id, ref,
                    deployment_items)
               VALUES ($1, $2, $3, '{}', '{}', '{}', $4,
                       jsonb_build_object('commit_branch', 'main'), $5)"#,
            annotation,
            format!("{slug}-{id}"),
            artifact,
            self.project_id,
            serde_json::json!([{
                "env": "dev",
                "destination": "^grund-pakkelyst-dev-",
                "destination_type": "forest/generic@1",
            }]),
        )
        .execute(&self.f.db)
        .await
        .expect("insert annotation");
        id
    }

    async fn release(&self, intent: Uuid, stage: &str, mode: &str, status: &str) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query!(
            r#"INSERT INTO release_states
                   (release_id, release_intent_id, project_id, destination_id,
                    artifact_id, status, stage_id, mode, plan_output, error_message)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
            id,
            intent,
            self.project_id,
            self.destination_id,
            Uuid::now_v7(),
            status,
            stage,
            mode,
            (mode == "plan" && status == "SUCCEEDED").then_some("{\"stack\":{}}"),
            (status == "FAILED").then(|| format!("{STALE_PLAN_PREFIX}{STALE}")),
        )
        .execute(&self.f.db)
        .await
        .expect("insert release");
        id
    }

    async fn set_stages(&self, intent: Uuid, plan: Uuid, deploy: Option<Uuid>) {
        let deploy_state = match deploy {
            Some(id) => serde_json::json!({ "status": "ACTIVE", "release_ids": [id.to_string()] }),
            None => serde_json::json!({ "status": "PENDING" }),
        };
        sqlx::query!(
            "UPDATE release_intents SET stage_states = $2 WHERE id = $1",
            intent,
            serde_json::json!({
                "plan-dev": {
                    "status": "SUCCEEDED", "approval_status": "APPROVED",
                    "release_ids": [plan.to_string()]
                },
                "deploy-dev": deploy_state,
            }),
        )
        .execute(&self.f.db)
        .await
        .expect("set stage states");
    }

    async fn planned_and_deploying(&self, slug: &str, deploy_status: &str) -> (Uuid, Uuid) {
        let intent = self.intent(slug).await;
        let plan = self.release(intent, "plan-dev", "plan", "SUCCEEDED").await;
        let deploy = self
            .release(intent, "deploy-dev", "deploy", deploy_status)
            .await;
        self.set_stages(intent, plan, Some(deploy)).await;
        (intent, deploy)
    }

    async fn stage(&self, intent: Uuid, stage: &str) -> serde_json::Value {
        sqlx::query_scalar!(
            r#"SELECT stage_states->$2 as "stage!" FROM release_intents WHERE id = $1"#,
            intent,
            stage,
        )
        .fetch_one(&self.f.db)
        .await
        .expect("stage state")
    }

    async fn slug(&self, intent: Uuid) -> String {
        sqlx::query_scalar!(
            "SELECT a.slug FROM release_intents ri JOIN annotations a ON a.id = ri.annotation_id
             WHERE ri.id = $1",
            intent,
        )
        .fetch_one(&self.f.db)
        .await
        .expect("slug")
    }

    async fn plan_waits(&self, release_id: Uuid) -> bool {
        let state = self
            .f
            .state
            .release_event_store()
            .get_release_state(&release_id)
            .await
            .expect("release state");
        plan_guard::waits_for_outstanding_plan(
            &self.f.db,
            &self.f.state.nats,
            &self.f.state.policy_registry(),
            &state,
        )
        .await
    }

    async fn evaluate(&self, intent: Uuid) {
        intent_coordinator::evaluate(&self.f.state, intent)
            .await
            .expect("evaluate intent");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plan_for_a_grund_stack_waits_until_the_earlier_release_has_deployed()
-> anyhow::Result<()> {
    let s = Scene::new().await?;
    let (earlier, deploy) = s
        .planned_and_deploying("cussedly-proven-fly", "RUNNING")
        .await;
    let later = s.intent("lamely-elevated-stargazer").await;
    let later_plan = s.release(later, "plan-dev", "plan", "QUEUED").await;

    assert!(
        s.plan_waits(later_plan).await,
        "planning now would seal a version the running deploy is about to move"
    );

    sqlx::query!(
        "UPDATE release_states SET status = 'SUCCEEDED' WHERE release_id = $1",
        deploy
    )
    .execute(&s.f.db)
    .await?;
    s.evaluate(earlier).await;
    assert_eq!(s.stage(earlier, "deploy-dev").await["status"], "SUCCEEDED");

    assert!(
        !s.plan_waits(later_plan).await,
        "once the earlier deploy is done, the later release plans against it"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_burst_of_stale_releases_leaves_the_newest_to_replan_and_deploy() -> anyhow::Result<()> {
    let s = Scene::new().await?;
    let (oldest, _) = s
        .planned_and_deploying("suddenly-foxy-firefly", "FAILED")
        .await;
    let (middle, _) = s
        .planned_and_deploying("regularly-fortified-sandfish", "FAILED")
        .await;
    let (newest, _) = s
        .planned_and_deploying("chastely-efficient-yellowhammer", "FAILED")
        .await;
    let newest_slug = s.slug(newest).await;

    for intent in [oldest, middle, newest] {
        s.evaluate(intent).await;
    }

    for older in [oldest, middle] {
        let deploy = s.stage(older, "deploy-dev").await;
        assert_eq!(deploy["status"], "SUPERSEDED", "{deploy}");
        let reason = deploy["error_message"].as_str().unwrap_or_default();
        assert!(
            reason.starts_with(&format!("superseded by release {newest_slug}")),
            "the reason names the release that replaced it: {reason}"
        );
    }

    let plan = s.stage(newest, "plan-dev").await;
    assert_eq!(plan["status"], "ACTIVE", "the newest plans again: {plan}");
    let deploy = s.stage(newest, "deploy-dev").await;
    assert_eq!(
        deploy["status"], "PENDING",
        "and deploys after the fresh plan: {deploy}"
    );
    assert!(
        deploy["error_message"]
            .as_str()
            .is_some_and(|m| m.starts_with("re-planning:")),
        "{deploy}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replanned_release_whose_destination_moves_again_fails_instead_of_looping()
-> anyhow::Result<()> {
    let s = Scene::new().await?;
    let intent = s.intent("expansively-deft-whale").await;
    s.release(intent, "deploy-dev", "deploy", "FAILED").await;
    let plan = s.release(intent, "plan-dev", "plan", "SUCCEEDED").await;
    let deploy = s.release(intent, "deploy-dev", "deploy", "FAILED").await;
    s.set_stages(intent, plan, Some(deploy)).await;

    s.evaluate(intent).await;

    let deploy = s.stage(intent, "deploy-dev").await;
    assert_eq!(deploy["status"], "FAILED", "{deploy}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deploy_that_failed_for_another_reason_still_fails() -> anyhow::Result<()> {
    let s = Scene::new().await?;
    let (intent, deploy) = s
        .planned_and_deploying("generally-grand-koala", "RUNNING")
        .await;
    sqlx::query!(
        "UPDATE release_states SET status = 'FAILED',
             error_message = 'provider reported failure: grund rolled the stack back after this revision failed'
         WHERE release_id = $1",
        deploy
    )
    .execute(&s.f.db)
    .await?;
    let _newer = s
        .planned_and_deploying("sadly-teaching-troll", "QUEUED")
        .await;

    s.evaluate(intent).await;

    assert_eq!(s.stage(intent, "deploy-dev").await["status"], "FAILED");
    Ok(())
}
