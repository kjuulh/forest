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
        self.saved_plan_with(intent, age_days, Some(true)).await;
    }

    async fn saved_no_op_plan(&self, intent: Uuid) {
        self.saved_plan_with(intent, 0, Some(false)).await;
    }

    async fn saved_plan_with(&self, intent: Uuid, age_days: i32, has_changes: Option<bool>) {
        sqlx::query!(
            r#"INSERT INTO terraform_plans
                   (release_intent_id, destination_id, work_dir, plan, created_at, has_changes)
               VALUES ($1, $2, '.', '\x00', now() - make_interval(days => $3), $4)"#,
            intent,
            self.destination_id,
            age_days,
            has_changes,
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

    async fn release_state(
        &self,
        release_id: Uuid,
    ) -> forest_server::services::release_event_store::ReleaseState {
        self.f
            .state
            .release_event_store()
            .get_release_state(&release_id)
            .await
            .expect("release state")
    }

    async fn waits(&self, release_id: Uuid) -> bool {
        let state = self.release_state(release_id).await;
        plan_guard::waits_for_outstanding_plan(
            &self.f.db,
            &self.f.state.nats,
            &self.f.state.policy_registry(),
            &state,
        )
        .await
    }

    async fn wait_reason(&self, release_id: Uuid) -> Option<String> {
        let state = self.release_state(release_id).await;
        plan_guard::wait_reason(&self.f.db, &state).await
    }

    async fn destination_name(&self) -> String {
        sqlx::query_scalar!(
            "SELECT name FROM destinations WHERE id = $1",
            self.destination_id
        )
        .fetch_one(&self.f.db)
        .await
        .expect("destination")
    }

    async fn annotated_intent(
        &self,
        stage_states: serde_json::Value,
        slug: &str,
        branch: &str,
    ) -> Uuid {
        let intent = self.intent(stage_states).await;
        sqlx::query!(
            r#"INSERT INTO annotations
                   (id, slug, artifact_id, metadata, source, context, project_id, ref)
               SELECT ri.annotation_id, $2, ri.artifact, '{}', '{}', '{}', ri.project_id,
                      jsonb_build_object('commit_branch', $3::text)
               FROM release_intents ri WHERE ri.id = $1"#,
            intent,
            format!("{slug}-{}", Uuid::now_v7()),
            branch,
        )
        .execute(&self.f.db)
        .await
        .expect("insert annotation");
        intent
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

    async fn plan_stage(&self, intent: Uuid) -> serde_json::Value {
        sqlx::query_scalar!(
            r#"SELECT stage_states->'plan-prod' as "stage!" FROM release_intents WHERE id = $1"#,
            intent,
        )
        .fetch_one(&self.f.db)
        .await
        .expect("plan stage")
    }

    async fn has_saved_plan(&self, intent: Uuid) -> bool {
        sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM terraform_plans
                WHERE release_intent_id = $1 AND destination_id = $2) as "exists!""#,
            intent,
            self.destination_id,
        )
        .fetch_one(&self.f.db)
        .await
        .expect("saved plan lookup")
    }

    async fn enable_supersede_pending_same_branch(&self) {
        sqlx::query!(
            r#"INSERT INTO policies (project_id, name, policy_type, config)
               VALUES ($1, 'collapse-prod', 'supersede_pending', $2)"#,
            self.project_id,
            serde_json::json!({
                "target_environment": self.environment,
                "same_branch_only": true,
                "cancel_in_progress": false,
            }),
        )
        .execute(&self.f.db)
        .await
        .expect("create supersede_pending policy");
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
    s.set_stage_states(a, stage_states(approved(), "PENDING"))
        .await;
    assert!(
        s.waits(b_plan).await,
        "then waiting for A to apply the plan it saved and had approved"
    );

    s.cleanup().await;
    Ok(())
}

fn awaiting_approval() -> serde_json::Value {
    serde_json::json!({ "status": "ACTIVE", "approval_status": "AWAITING_APPROVAL" })
}

async fn parked_no_op_plans_never_hold_a_new_plan() -> anyhow::Result<()> {
    let s = Scene::new().await?;

    let mut parked = Vec::new();
    for i in 0..4 {
        let a = s.intent(stage_states(awaiting_approval(), "PENDING")).await;
        s.release(a, "plan-prod", "plan", "SUCCEEDED", -3600 + i)
            .await;
        s.saved_no_op_plan(a).await;
        parked.push(a);
    }

    let b = s.intent(stage_states(planning(), "PENDING")).await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;

    assert_eq!(s.wait_reason(b_plan).await, None);
    assert!(
        !s.waits(b_plan).await,
        "four parked 'No changes' plans must not hold the destination"
    );
    assert_eq!(s.waiting_lines(b_plan).await, 0);
    for a in parked {
        let stage = s.plan_stage(a).await;
        assert_eq!(stage["stale_by"], serde_json::json!(b.to_string()));
        assert_eq!(stage["approval_status"], "AWAITING_APPROVAL");
        assert!(
            stage["error_message"]
                .as_str()
                .is_some_and(|m| m.starts_with("stale: release ")),
            "{stage}"
        );
        assert!(
            !s.has_saved_plan(a).await,
            "the stale plan file is discarded"
        );
    }

    s.cleanup().await;
    Ok(())
}

async fn an_approved_no_op_plan_does_not_hold() -> anyhow::Result<()> {
    let s = Scene::new().await?;

    let a = s.intent(stage_states(approved(), "PENDING")).await;
    s.release(a, "plan-prod", "plan", "SUCCEEDED", -60).await;
    s.saved_no_op_plan(a).await;

    let b = s.intent(stage_states(planning(), "PENDING")).await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;

    assert!(!s.waits(b_plan).await);
    assert!(
        s.has_saved_plan(a).await,
        "an approved plan is left for its own deploy to apply"
    );

    s.cleanup().await;
    Ok(())
}

async fn a_plan_saved_before_changes_were_recorded_still_holds() -> anyhow::Result<()> {
    let s = Scene::new().await?;

    let a = s.intent(stage_states(approved(), "PENDING")).await;
    s.release(a, "plan-prod", "plan", "SUCCEEDED", -60).await;
    s.saved_plan_with(a, 0, None).await;

    let b = s.intent(stage_states(planning(), "PENDING")).await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;

    assert!(s.waits(b_plan).await);

    s.cleanup().await;
    Ok(())
}

async fn an_unapproved_older_plan_is_marked_stale_without_a_policy() -> anyhow::Result<()> {
    let s = Scene::new().await?;

    let a = s
        .annotated_intent(
            stage_states(awaiting_approval(), "PENDING"),
            "old-otter",
            "main",
        )
        .await;
    s.release(a, "plan-prod", "plan", "SUCCEEDED", -60).await;
    s.saved_plan(a, 0).await;

    let b = s
        .annotated_intent(stage_states(planning(), "PENDING"), "new-heron", "main")
        .await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;

    assert!(!s.waits(b_plan).await, "B plans at once");

    let b_slug = s.slug(b).await;
    let dest = s.destination_name().await;
    let stage = s.plan_stage(a).await;
    assert_eq!(stage["status"], "ACTIVE");
    assert_eq!(
        stage["error_message"],
        serde_json::json!(plan_guard::stale_reason(&b_slug, &dest))
    );
    assert!(!s.has_saved_plan(a).await);

    assert!(!s.waits(b_plan).await, "re-checking changes nothing");

    s.cleanup().await;
    Ok(())
}

async fn an_unapproved_older_plan_is_superseded_under_the_policy() -> anyhow::Result<()> {
    let s = Scene::new().await?;
    s.enable_supersede_pending().await;

    let a = s
        .annotated_intent(
            stage_states(awaiting_approval(), "PENDING"),
            "old-otter",
            "main",
        )
        .await;
    s.release(a, "plan-prod", "plan", "SUCCEEDED", -60).await;
    s.saved_plan(a, 0).await;

    let b = s
        .annotated_intent(
            stage_states(planning(), "PENDING"),
            "new-heron",
            "renovate/x",
        )
        .await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;

    assert!(!s.waits(b_plan).await);

    let b_slug = s.slug(b).await;
    let dest = s.destination_name().await;
    let stage = s.plan_stage(a).await;
    assert_eq!(stage["status"], "SUPERSEDED");
    assert!(stage.get("approval_status").is_none(), "{stage}");
    assert_eq!(
        stage["error_message"],
        serde_json::json!(plan_guard::superseded_reason(
            &b_slug,
            &dest,
            "collapse-prod"
        ))
    );
    assert!(!s.has_saved_plan(a).await);

    s.cleanup().await;
    Ok(())
}

async fn a_same_branch_policy_only_marks_another_branch_stale() -> anyhow::Result<()> {
    let s = Scene::new().await?;
    s.enable_supersede_pending_same_branch().await;

    let same = s
        .annotated_intent(stage_states(awaiting_approval(), "PENDING"), "same", "main")
        .await;
    s.release(same, "plan-prod", "plan", "SUCCEEDED", -60).await;
    s.saved_plan(same, 0).await;
    let other = s
        .annotated_intent(
            stage_states(awaiting_approval(), "PENDING"),
            "other",
            "feature/y",
        )
        .await;
    s.release(other, "plan-prod", "plan", "SUCCEEDED", -50)
        .await;
    s.saved_plan(other, 0).await;

    let b = s
        .annotated_intent(stage_states(planning(), "PENDING"), "newest", "main")
        .await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;

    assert!(!s.waits(b_plan).await);
    assert_eq!(s.plan_stage(same).await["status"], "SUPERSEDED");
    let other_stage = s.plan_stage(other).await;
    assert_eq!(other_stage["status"], "ACTIVE");
    assert_eq!(other_stage["stale_by"], serde_json::json!(b.to_string()));

    s.cleanup().await;
    Ok(())
}

async fn the_wait_names_the_release_holding_the_destination() -> anyhow::Result<()> {
    let s = Scene::new().await?;

    let a = s
        .annotated_intent(stage_states(approved(), "PENDING"), "brave-otter", "main")
        .await;
    s.release(a, "plan-prod", "plan", "SUCCEEDED", -60).await;
    s.saved_plan(a, 0).await;

    let b = s.intent(stage_states(planning(), "PENDING")).await;
    let b_plan = s.release(b, "plan-prod", "plan", "QUEUED", 0).await;

    let a_slug = s.slug(a).await;
    let dest = s.destination_name().await;
    assert_eq!(
        s.wait_reason(b_plan).await,
        Some(format!(
            "waiting for release {a_slug} to apply its plan for {dest} before planning"
        ))
    );
    assert_eq!(
        s.plan_stage(a).await,
        approved(),
        "showing the wait changes nothing"
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

    parked_no_op_plans_never_hold_a_new_plan()
        .await
        .context("parked no-op plans never hold a new plan")?;

    an_approved_no_op_plan_does_not_hold()
        .await
        .context("an approved no-op plan does not hold")?;

    a_plan_saved_before_changes_were_recorded_still_holds()
        .await
        .context("a plan saved before changes were recorded still holds")?;

    an_unapproved_older_plan_is_marked_stale_without_a_policy()
        .await
        .context("an unapproved older plan is marked stale without a policy")?;

    an_unapproved_older_plan_is_superseded_under_the_policy()
        .await
        .context("an unapproved older plan is superseded under the policy")?;

    a_same_branch_policy_only_marks_another_branch_stale()
        .await
        .context("a same-branch policy only marks another branch stale")?;

    the_wait_names_the_release_holding_the_destination()
        .await
        .context("the wait names the release holding the destination")?;

    Ok(())
}
