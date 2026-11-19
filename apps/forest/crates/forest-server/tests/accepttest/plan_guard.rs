use forest_server::services::plan_guard;
use forest_server::services::policy::PolicyRegistryState;
use forest_server::services::release_event_store::ReleaseEventStoreState;
use forest_server::services::supersede::{self, Verdict};
use uuid::Uuid;

use crate::accepttest::fixtures::{Fixture, fixture};

struct Scene {
    f: Fixture,
    project_id: Uuid,
    destination_id: Uuid,
    environment: String,
}

fn pipeline() -> serde_json::Value {
    serde_json::json!({
        "deploy-dev": { "type": "deploy", "environment": "dev", "depends_on": [] },
        "plan-prod": { "type": "plan", "environment": "prod", "depends_on": [] },
        "deploy-prod": {
            "type": "deploy",
            "environment": "prod",
            "depends_on": ["plan-prod", "deploy-dev"]
        }
    })
}

fn stage_states(plan_prod: serde_json::Value, deploy_prod: &str) -> serde_json::Value {
    serde_json::json!({
        "deploy-dev": { "status": "ACTIVE" },
        "plan-prod": plan_prod,
        "deploy-prod": { "status": deploy_prod }
    })
}

fn approved() -> serde_json::Value {
    serde_json::json!({ "status": "SUCCEEDED", "approval_status": "APPROVED" })
}

fn planning() -> serde_json::Value {
    serde_json::json!({ "status": "ACTIVE" })
}

impl Scene {
    async fn new() -> anyhow::Result<Self> {
        Self::with_type("forest", "terraform").await
    }

    async fn with_type(type_organisation: &str, type_name: &str) -> anyhow::Result<Self> {
        let f = fixture().await?;
        let suffix = Uuid::now_v7();
        let org = format!("plan-guard-org-{suffix}");
        let environment = format!("plan-guard-env-{suffix}");

        sqlx::query!(
            "INSERT INTO organisations (id, name) VALUES ($1, $2)",
            Uuid::now_v7(),
            org,
        )
        .execute(&f.db)
        .await?;
        let environment_id = sqlx::query_scalar!(
            "INSERT INTO environments (organisation, name) VALUES ($1, $2) RETURNING id",
            org,
            environment,
        )
        .fetch_one(&f.db)
        .await?;
        let destination_id = sqlx::query_scalar!(
            r#"INSERT INTO destinations
                   (name, environment, organisation, type_organisation, type_name,
                    type_version, metadata, environment_id)
               VALUES ($1, $2, $3, $4, $5, 1, '{}', $6)
               RETURNING id"#,
            format!("prod-{suffix}"),
            environment,
            org,
            type_organisation,
            type_name,
            environment_id,
        )
        .fetch_one(&f.db)
        .await?;
        let project_id = sqlx::query_scalar!(
            "INSERT INTO projects (organisation, project) VALUES ($1, 'agent-gateway') RETURNING id",
            org,
        )
        .fetch_one(&f.db)
        .await?;

        Ok(Self {
            f,
            project_id,
            destination_id,
            environment,
        })
    }

    async fn enable_supersede_pending(&self) {
        sqlx::query!(
            r#"INSERT INTO policies (project_id, name, policy_type, config)
               VALUES ($1, 'collapse-prod', 'supersede_pending', $2)"#,
            self.project_id,
            serde_json::json!({
                "target_environment": self.environment,
                "same_branch_only": false,
                "cancel_in_progress": false,
            }),
        )
        .execute(&self.f.db)
        .await
        .expect("create supersede_pending policy");
    }

    async fn intent(&self, stage_states: serde_json::Value) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query!(
            r#"INSERT INTO release_intents
                   (id, artifact, annotation_id, project_id, status, stages, stage_states)
               VALUES ($1, $2, $3, $4, 'ACTIVE', $5, $6)"#,
            id,
            Uuid::now_v7(),
            Uuid::now_v7(),
            self.project_id,
            pipeline(),
            stage_states,
        )
        .execute(&self.f.db)
        .await
        .expect("insert intent");
        id
    }

    async fn release(
        &self,
        intent: Uuid,
        stage: &str,
        mode: &str,
        status: &str,
        queued_offset_secs: i64,
    ) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query!(
            r#"INSERT INTO release_states
                   (release_id, release_intent_id, project_id, destination_id,
                    artifact_id, status, stage_id, mode, queued_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8,
                       now() + ($9 || ' seconds')::interval)"#,
            id,
            intent,
            self.project_id,
            self.destination_id,
            Uuid::now_v7(),
            status,
            stage,
            mode,
            queued_offset_secs.to_string(),
        )
        .execute(&self.f.db)
        .await
        .expect("insert release");
        id
    }

    async fn saved_plan(&self, intent: Uuid, age_days: i32) {
        sqlx::query!(
            r#"INSERT INTO terraform_plans
                   (release_intent_id, destination_id, work_dir, plan, created_at)
               VALUES ($1, $2, '.', '\x00', now() - make_interval(days => $3))"#,
            intent,
            self.destination_id,
            age_days,
        )
        .execute(&self.f.db)
        .await
        .expect("insert saved plan");
    }

    async fn discard_plan(&self, intent: Uuid) {
        sqlx::query!(
            "DELETE FROM terraform_plans WHERE release_intent_id = $1 AND destination_id = $2",
            intent,
            self.destination_id,
        )
        .execute(&self.f.db)
        .await
        .expect("discard plan");
    }

    async fn set_status(&self, release_id: Uuid, status: &str) {
        sqlx::query!(
            "UPDATE release_states SET status = $2 WHERE release_id = $1",
            release_id,
            status,
        )
        .execute(&self.f.db)
        .await
        .expect("update release");
    }

    async fn status(&self, release_id: Uuid) -> String {
        sqlx::query_scalar!(
            "SELECT status FROM release_states WHERE release_id = $1",
            release_id
        )
        .fetch_one(&self.f.db)
        .await
        .expect("release row")
    }

    async fn set_stage_states(&self, intent: Uuid, stage_states: serde_json::Value) {
        sqlx::query!(
            "UPDATE release_intents SET stage_states = $2 WHERE id = $1",
            intent,
            stage_states,
        )
        .execute(&self.f.db)
        .await
        .expect("update stage states");
    }

    async fn waits(&self, release_id: Uuid) -> bool {
        let state = self
            .f
            .state
            .release_event_store()
            .get_release_state(&release_id)
            .await
            .expect("release state");
        plan_guard::waits_for_outstanding_plan(&self.f.db, &state).await
    }

    async fn collapse(&self, candidate: Uuid) -> Verdict {
        supersede::collapse_queue(
            &self.f.state.release_event_store(),
            &self.f.state.policy_registry(),
            &self.f.state.nats,
            candidate,
            self.project_id,
            self.destination_id,
            &self.environment,
        )
        .await
        .expect("collapse queue")
    }

    async fn last_touched(&self, release_id: Uuid, secs_ago: i64) {
        sqlx::query!(
            "UPDATE release_states
                SET updated_at = now() - ($2 || ' seconds')::interval,
                    queued_at = now() - ($2 || ' seconds')::interval
              WHERE release_id = $1",
            release_id,
            secs_ago.to_string(),
        )
        .execute(&self.f.db)
        .await
        .expect("age release");
    }

    async fn waiting_lines(&self, release_id: Uuid) -> i64 {
        sqlx::query_scalar!(
            r#"SELECT count(*) as "count!"
               FROM release_states rs
               JOIN release_logs rl
                 ON rl.release_intent_id = rs.release_intent_id
                AND rl.destination_id = rs.destination_id
               CROSS JOIN jsonb_array_elements(rl.log_lines) line
               WHERE rs.release_id = $1
                 AND rl.release_attempt <> rs.release_id
                 AND rl.sequence = 0
                 AND line->>'line' LIKE 'forest: waiting for release % before planning'"#,
            release_id,
        )
        .fetch_one(&self.f.db)
        .await
        .expect("count log lines")
    }

    async fn cleanup(&self) {
        let _ = sqlx::query!(
            "UPDATE release_states SET status = 'CANCELLED'
              WHERE project_id = $1 AND status IN ('QUEUED', 'ASSIGNED', 'RUNNING')",
            self.project_id,
        )
        .execute(&self.f.db)
        .await;
        let _ = sqlx::query!(
            "UPDATE release_intents SET status = 'CANCELLED' WHERE project_id = $1",
            self.project_id,
        )
        .execute(&self.f.db)
        .await;
        let _ = sqlx::query!(
            "DELETE FROM terraform_plans WHERE destination_id = $1",
            self.destination_id,
        )
        .execute(&self.f.db)
        .await;
        let _ = sqlx::query!(
            "DELETE FROM policies WHERE project_id = $1",
            self.project_id,
        )
        .execute(&self.f.db)
        .await;
    }
}

async fn a_burst_plan_waits_until_the_earlier_release_applies() -> anyhow::Result<()> {
    let s = Scene::new().await?;

    let a = s.intent(stage_states(approved(), "PENDING")).await;
    s.release(a, "plan-prod", "plan", "SUCCEEDED", -60).await;
    s.saved_plan(a, 0).await;

    let b = s.intent(stage_states(planning(), "PENDING")).await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;

    assert!(
        s.waits(b_plan).await,
        "A's plan is approved but not applied"
    );
    assert!(s.waits(b_plan).await, "re-checking keeps it waiting");
    assert_eq!(
        s.waiting_lines(b_plan).await,
        1,
        "the waiting line is written once, not on every re-check"
    );

    s.set_stage_states(a, stage_states(approved(), "ACTIVE"))
        .await;
    let a_deploy = s.release(a, "deploy-prod", "deploy", "RUNNING", 30).await;
    assert!(s.waits(b_plan).await, "A is applying its plan");

    s.set_status(a_deploy, "SUCCEEDED").await;
    s.discard_plan(a).await;
    assert!(
        !s.waits(b_plan).await,
        "A applied, B plans against fresh state"
    );

    s.cleanup().await;
    Ok(())
}

async fn a_held_plan_is_not_superseded_by_the_deploy_it_waits_for() -> anyhow::Result<()> {
    let s = Scene::new().await?;
    s.enable_supersede_pending().await;

    let a = s.intent(stage_states(approved(), "PENDING")).await;
    s.release(a, "plan-prod", "plan", "SUCCEEDED", -60).await;
    s.saved_plan(a, 0).await;

    let b = s.intent(stage_states(planning(), "PENDING")).await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;
    assert!(s.waits(b_plan).await, "B's plan is held behind A's");

    s.set_stage_states(a, stage_states(approved(), "ACTIVE"))
        .await;
    let a_deploy = s.release(a, "deploy-prod", "deploy", "QUEUED", 30).await;

    assert_eq!(
        s.collapse(a_deploy).await,
        Verdict::Proceed,
        "A's deploy is the only queued deploy"
    );
    assert_eq!(
        s.status(b_plan).await,
        "QUEUED",
        "a queued deploy must not supersede a queued plan"
    );
    assert_eq!(
        s.collapse(b_plan).await,
        Verdict::Proceed,
        "B's plan is the only queued plan"
    );
    assert_eq!(s.status(b_plan).await, "QUEUED");
    assert_eq!(s.status(a_deploy).await, "QUEUED");

    s.set_status(a_deploy, "SUCCEEDED").await;
    s.discard_plan(a).await;
    assert!(
        !s.waits(b_plan).await,
        "B plans once A's deploy has finished"
    );

    s.cleanup().await;
    Ok(())
}

async fn queued_plans_still_collapse_to_the_newest_plan() -> anyhow::Result<()> {
    let s = Scene::new().await?;
    s.enable_supersede_pending().await;

    let b = s.intent(stage_states(planning(), "PENDING")).await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;
    let c = s.intent(stage_states(planning(), "PENDING")).await;
    let c_plan = s.release(c, "plan-prod", "plan", "QUEUED", 10).await;

    assert_eq!(s.collapse(b_plan).await, Verdict::Superseded);
    assert_eq!(s.status(b_plan).await, "SUPERSEDED");
    assert_eq!(s.status(c_plan).await, "QUEUED");

    s.cleanup().await;
    Ok(())
}

async fn an_earlier_plan_in_flight_holds_a_later_one() -> anyhow::Result<()> {
    let s = Scene::new().await?;

    let a = s.intent(stage_states(planning(), "PENDING")).await;
    let a_plan = s.release(a, "plan-prod", "plan", "QUEUED", -10).await;
    let b = s.intent(stage_states(planning(), "PENDING")).await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;

    assert!(!s.waits(a_plan).await, "the earlier plan goes first");
    assert!(s.waits(b_plan).await, "the later plan waits for it");

    s.set_status(a_plan, "RUNNING").await;
    assert!(s.waits(b_plan).await, "still waiting while A plans");

    s.set_status(a_plan, "SUCCEEDED").await;
    s.saved_plan(a, 0).await;
    assert!(
        s.waits(b_plan).await,
        "then waiting for A to apply the plan it saved"
    );

    s.cleanup().await;
    Ok(())
}

async fn a_plan_older_than_the_prune_age_does_not_block() -> anyhow::Result<()> {
    let s = Scene::new().await?;

    let a = s.intent(stage_states(approved(), "PENDING")).await;
    s.release(a, "plan-prod", "plan", "SUCCEEDED", -60).await;
    s.saved_plan(a, 8).await;

    let b = s.intent(stage_states(planning(), "PENDING")).await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;

    assert!(
        !s.waits(b_plan).await,
        "a plan past the 7-day prune age never holds, even if no save has pruned it yet"
    );

    s.cleanup().await;
    Ok(())
}

async fn a_rejected_plan_does_not_hold_up_the_next_one() -> anyhow::Result<()> {
    let s = Scene::new().await?;

    let a = s
        .intent(stage_states(
            serde_json::json!({ "status": "FAILED", "approval_status": "REJECTED" }),
            "CANCELLED",
        ))
        .await;
    s.release(a, "plan-prod", "plan", "SUCCEEDED", -60).await;
    s.saved_plan(a, 0).await;

    let b = s.intent(stage_states(planning(), "PENDING")).await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;

    assert!(
        !s.waits(b_plan).await,
        "a rejected plan keeps its row until pruned but will never be applied"
    );
    assert_eq!(s.waiting_lines(b_plan).await, 0);

    s.cleanup().await;
    Ok(())
}

async fn a_deploy_release_never_waits() -> anyhow::Result<()> {
    let s = Scene::new().await?;

    let a = s.intent(stage_states(approved(), "PENDING")).await;
    s.release(a, "plan-prod", "plan", "SUCCEEDED", -60).await;
    s.saved_plan(a, 0).await;

    let b = s.intent(stage_states(approved(), "ACTIVE")).await;
    let b_deploy = s.release(b, "deploy-prod", "deploy", "QUEUED", 0).await;

    assert!(
        !s.waits(b_deploy).await,
        "a deploy stage applies its own approved plan"
    );

    s.cleanup().await;
    Ok(())
}

async fn a_destination_that_saves_no_plans_never_waits() -> anyhow::Result<()> {
    let s = Scene::with_type("forest", "kubernetes").await?;

    let a = s.intent(stage_states(planning(), "PENDING")).await;
    s.release(a, "plan-prod", "plan", "RUNNING", -10).await;
    let b = s.intent(stage_states(planning(), "PENDING")).await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;

    assert!(!s.waits(b_plan).await);

    s.cleanup().await;
    Ok(())
}

async fn held_plans_cannot_crowd_other_releases_out_of_the_sweep() -> anyhow::Result<()> {
    let s = Scene::new().await?;
    let other = Scene::with_type("forest", "kubernetes").await?;

    let a = s.intent(stage_states(approved(), "PENDING")).await;
    s.release(a, "plan-prod", "plan", "SUCCEEDED", -7200).await;
    s.saved_plan(a, 0).await;

    let mut held = Vec::new();
    for i in 0..12 {
        let b = s.intent(stage_states(planning(), "PENDING")).await;
        let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;
        s.last_touched(b_plan, 3600 - i).await;
        held.push(b_plan);
    }

    let o = other.intent(stage_states(approved(), "ACTIVE")).await;
    let other_deploy = other.release(o, "deploy-prod", "deploy", "QUEUED", 0).await;
    other.last_touched(other_deploy, 60).await;

    for b_plan in &held {
        assert!(s.waits(*b_plan).await, "every B is held behind A");
    }

    let picked: Vec<Uuid> =
        s.f.state
            .release_event_store()
            .pick_queued_releases(10)
            .await?
            .into_iter()
            .map(|r| r.release_id)
            .collect();
    assert!(
        picked.contains(&other_deploy),
        "another project's queued release is still swept while older plans are held"
    );

    s.cleanup().await;
    other.cleanup().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plan_waits_for_outstanding_plan() -> anyhow::Result<()> {
    use anyhow::Context;

    a_burst_plan_waits_until_the_earlier_release_applies()
        .await
        .context("a burst plan waits until the earlier release applies")?;

    a_held_plan_is_not_superseded_by_the_deploy_it_waits_for()
        .await
        .context("a held plan is not superseded by the deploy it waits for")?;

    queued_plans_still_collapse_to_the_newest_plan()
        .await
        .context("queued plans still collapse to the newest plan")?;

    an_earlier_plan_in_flight_holds_a_later_one()
        .await
        .context("an earlier plan in flight holds a later one")?;

    a_plan_older_than_the_prune_age_does_not_block()
        .await
        .context("a plan older than the prune age does not block")?;

    a_rejected_plan_does_not_hold_up_the_next_one()
        .await
        .context("a rejected plan does not hold up the next one")?;

    a_deploy_release_never_waits()
        .await
        .context("a deploy release never waits")?;

    a_destination_that_saves_no_plans_never_waits()
        .await
        .context("a destination that saves no plans never waits")?;

    held_plans_cannot_crowd_other_releases_out_of_the_sweep()
        .await
        .context("held plans cannot crowd other releases out of the sweep")?;

    Ok(())
}
