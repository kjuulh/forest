use forest_server::destinations::genericv1::STALE_PLAN_PREFIX;
use forest_server::intent_coordinator;
use forest_server::services::newest_wins;
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
        let s = Self::unguarded().await?;
        s.hold_the_background_scheduler().await?;
        Ok(s)
    }

    async fn hold_the_background_scheduler(&self) -> anyhow::Result<()> {
        let sentinel = Uuid::now_v7();
        sqlx::query!(
            r#"INSERT INTO release_intents (id, artifact, annotation_id, project_id, status)
               VALUES ($1, $2, $3, $4, 'CANCELLED')"#,
            sentinel,
            Uuid::now_v7(),
            Uuid::now_v7(),
            self.project_id,
        )
        .execute(&self.f.db)
        .await?;
        sqlx::query!(
            r#"INSERT INTO release_states
                   (release_id, release_intent_id, project_id, destination_id,
                    artifact_id, status, mode)
               VALUES ($1, $2, $3, $4, $5, 'RUNNING', 'deploy')"#,
            Uuid::now_v7(),
            sentinel,
            self.project_id,
            self.destination_id,
            Uuid::now_v7(),
        )
        .execute(&self.f.db)
        .await?;
        Ok(())
    }

    async fn unguarded() -> anyhow::Result<Self> {
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
    let s = Scene::unguarded().await?;
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
    let s = Scene::unguarded().await?;
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

#[tokio::test(flavor = "multi_thread")]
async fn a_replanning_release_and_an_older_queued_plan_never_wait_on_each_other()
-> anyhow::Result<()> {
    let s = Scene::new().await?;
    let c = s.intent("older-queued-c").await;
    let (a, _) = s.planned_and_deploying("newer-stale-a", "FAILED").await;
    let c_plan = s.release(c, "plan-dev", "plan", "QUEUED").await;
    sqlx::query("UPDATE release_intents SET created = now() - interval '1 hour' WHERE id = $1")
        .bind(c)
        .execute(&s.f.db)
        .await?;
    sqlx::query("UPDATE release_intents SET stage_states = $2 WHERE id = $1")
        .bind(c)
        .bind(serde_json::json!({
            "plan-dev": { "status": "ACTIVE", "release_ids": [c_plan.to_string()] },
            "deploy-dev": { "status": "PENDING" },
        }))
        .execute(&s.f.db)
        .await?;

    s.evaluate(a).await;
    let deploy = s.stage(a, "deploy-dev").await;
    assert_eq!(
        deploy["status"], "PENDING",
        "A replans (no newer release): {deploy}"
    );
    let a_new_plan: Uuid = sqlx::query_scalar(
        "SELECT release_id FROM release_states WHERE release_intent_id = $1 AND mode = 'plan' AND status = 'QUEUED'",
    )
    .bind(a)
    .fetch_one(&s.f.db)
    .await?;

    let c_waits = s.plan_waits(c_plan).await;
    let a_waits = s.plan_waits(a_new_plan).await;
    assert!(
        !(c_waits && a_waits),
        "DEADLOCK: C's plan waits on A's previous sealed plan, A's new plan waits on C planning ahead"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_newer_release_that_failed_does_not_supersede() -> anyhow::Result<()> {
    let s = Scene::new().await?;
    let (a, _) = s.planned_and_deploying("stale-a", "FAILED").await;
    let (b, b_deploy) = s.planned_and_deploying("failed-b", "FAILED").await;
    sqlx::query("UPDATE release_states SET error_message = 'provider reported failure: boom' WHERE release_id = $1")
        .bind(b_deploy).execute(&s.f.db).await?;
    sqlx::query("UPDATE release_intents SET status = 'FAILED' WHERE id = $1")
        .bind(b)
        .execute(&s.f.db)
        .await?;
    s.evaluate(a).await;
    let deploy = s.stage(a, "deploy-dev").await;
    assert_ne!(
        deploy["status"], "SUPERSEDED",
        "superseded by a FAILED release: {deploy}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn two_stale_destinations_in_one_attempt_still_replan_once() -> anyhow::Result<()> {
    let s = Scene::new().await?;
    let intent = s.intent("two-destinations").await;
    let plan = s.release(intent, "plan-dev", "plan", "SUCCEEDED").await;
    let first = s.release(intent, "deploy-dev", "deploy", "FAILED").await;
    let second = s.release(intent, "deploy-dev", "deploy", "FAILED").await;
    sqlx::query(
        "UPDATE release_states SET queued_at = (SELECT queued_at FROM release_states WHERE release_id = $1)
         WHERE release_id = $2",
    )
    .bind(first)
    .bind(second)
    .execute(&s.f.db)
    .await?;
    sqlx::query("UPDATE release_intents SET stage_states = $2 WHERE id = $1")
        .bind(intent)
        .bind(serde_json::json!({
            "plan-dev": {
                "status": "SUCCEEDED", "approval_status": "APPROVED",
                "release_ids": [plan.to_string()]
            },
            "deploy-dev": {
                "status": "ACTIVE",
                "release_ids": [first.to_string(), second.to_string()]
            },
        }))
        .execute(&s.f.db)
        .await?;

    s.evaluate(intent).await;

    let deploy = s.stage(intent, "deploy-dev").await;
    assert_eq!(
        deploy["status"], "PENDING",
        "one attempt, so it replans: {deploy}"
    );
    Ok(())
}

impl Scene {
    async fn retire_if_overtaken(&self, release_id: Uuid) -> newest_wins::Overtaken {
        self.retire_if_overtaken_on(release_id, None).await
    }

    async fn retire_if_overtaken_on(
        &self,
        release_id: Uuid,
        same_branch: Option<&str>,
    ) -> newest_wins::Overtaken {
        let state = self
            .f
            .state
            .release_event_store()
            .get_release_state(&release_id)
            .await
            .expect("release state");
        newest_wins::retire_if_overtaken(
            &self.f.state.release_event_store(),
            &self.f.state.nats,
            &state,
            "grund-pakkelyst-dev",
            same_branch,
        )
        .await
        .expect("newest-wins check")
    }

    async fn state_of(
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

    async fn updated_at(
        &self,
        ids: &[Uuid],
    ) -> std::collections::HashMap<Uuid, chrono::DateTime<chrono::Utc>> {
        sqlx::query_as::<_, (Uuid, chrono::DateTime<chrono::Utc>)>(
            "SELECT release_id, updated_at FROM release_states WHERE release_id = ANY($1)",
        )
        .bind(ids)
        .fetch_all(&self.f.db)
        .await
        .expect("updated_at")
        .into_iter()
        .collect()
    }

    async fn release_status(&self, release_id: Uuid) -> String {
        sqlx::query_scalar!(
            "SELECT status FROM release_states WHERE release_id = $1",
            release_id
        )
        .fetch_one(&self.f.db)
        .await
        .expect("release status")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_older_deploy_freed_from_a_backlog_never_lands_over_a_newer_one() -> anyhow::Result<()> {
    let s = Scene::new().await?;
    let (older, older_deploy) = s.planned_and_deploying("parked-by-soak", "QUEUED").await;
    let (newer, _) = s.planned_and_deploying("deployed-first", "SUCCEEDED").await;
    let newer_slug = s.slug(newer).await;

    assert_eq!(
        s.retire_if_overtaken(older_deploy).await,
        newest_wins::Overtaken::Superseded
    );
    assert_eq!(s.release_status(older_deploy).await, "SUPERSEDED");

    s.evaluate(older).await;
    let deploy = s.stage(older, "deploy-dev").await;
    assert_eq!(deploy["status"], "SUPERSEDED", "{deploy}");
    assert!(
        deploy["error_message"]
            .as_str()
            .is_some_and(|m| m.starts_with(&format!("superseded by release {newer_slug}"))),
        "{deploy}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_older_deploy_waits_for_a_newer_one_and_still_lands_if_it_fails() -> anyhow::Result<()> {
    let s = Scene::new().await?;
    let (_, older_deploy) = s.planned_and_deploying("older-queued", "QUEUED").await;
    let (newer, newer_deploy) = s.planned_and_deploying("newer-queued", "QUEUED").await;
    let newer_slug = s.slug(newer).await;

    assert_eq!(
        s.retire_if_overtaken(newer_deploy).await,
        newest_wins::Overtaken::No,
        "the newest deploys first"
    );
    assert_eq!(
        s.retire_if_overtaken(older_deploy).await,
        newest_wins::Overtaken::WaitsFor(newer_slug),
        "the older waits rather than being retired on a deploy that may fail"
    );
    assert_eq!(s.release_status(older_deploy).await, "QUEUED");

    sqlx::query("UPDATE release_states SET status = 'FAILED' WHERE release_id = $1")
        .bind(newer_deploy)
        .execute(&s.f.db)
        .await?;
    assert_eq!(
        s.retire_if_overtaken(older_deploy).await,
        newest_wins::Overtaken::No,
        "with the newer deploy failed, the older one still lands"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn under_same_branch_only_a_newer_release_from_another_branch_never_overtakes()
-> anyhow::Result<()> {
    let s = Scene::new().await?;
    let (_, older_deploy) = s.planned_and_deploying("main-older", "QUEUED").await;
    let (_, _) = s.planned_and_deploying("hotfix-newer", "SUCCEEDED").await;
    sqlx::query(
        "UPDATE annotations SET ref = jsonb_build_object('commit_branch', 'hotfix/pager')
         WHERE slug LIKE 'hotfix-newer-%'",
    )
    .execute(&s.f.db)
    .await?;

    assert_eq!(
        s.retire_if_overtaken_on(older_deploy, Some("main")).await,
        newest_wins::Overtaken::No
    );
    assert_eq!(
        s.retire_if_overtaken(older_deploy).await,
        newest_wins::Overtaken::Superseded,
        "without the branch guard every newer release counts"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_newer_deploy_is_never_held_back_by_an_older_one_or_a_failed_newer_one()
-> anyhow::Result<()> {
    let s = Scene::new().await?;
    let (_, _) = s.planned_and_deploying("older-deployed", "SUCCEEDED").await;
    let (_, current) = s.planned_and_deploying("current", "QUEUED").await;
    let (_, _) = s.planned_and_deploying("newer-failed", "FAILED").await;

    assert_eq!(
        s.retire_if_overtaken(current).await,
        newest_wins::Overtaken::No
    );
    assert_eq!(s.release_status(current).await, "QUEUED");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deploy_waits_while_another_deploy_runs_on_the_same_stack() -> anyhow::Result<()> {
    let s = Scene::unguarded().await?;
    let (_, running) = s.planned_and_deploying("deploying-now", "RUNNING").await;
    let (_, next) = s.planned_and_deploying("next-in-line", "QUEUED").await;

    assert!(
        newest_wins::deploy_in_flight(&s.f.db, &s.state_of(next).await)
            .await?
            .is_some(),
        "two deploys never run against one stack at once"
    );
    assert!(
        newest_wins::deploy_in_flight(&s.f.db, &s.state_of(running).await)
            .await?
            .is_none()
    );

    sqlx::query("UPDATE release_states SET status = 'SUCCEEDED' WHERE release_id = $1")
        .bind(running)
        .execute(&s.f.db)
        .await?;
    assert!(
        newest_wins::deploy_in_flight(&s.f.db, &s.state_of(next).await)
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn waiting_older_deploys_never_starve_the_newest_in_the_sweep() -> anyhow::Result<()> {
    let s = Scene::new().await?;
    let mut olders = Vec::new();
    for i in 0..10 {
        let (_, d) = s
            .planned_and_deploying(&format!("older-{i}"), "QUEUED")
            .await;
        olders.push(d);
    }
    let (_, _newest) = s.planned_and_deploying("newest", "QUEUED").await;
    let before = s.updated_at(&olders).await;

    for d in &olders {
        assert!(matches!(
            s.retire_if_overtaken(*d).await,
            newest_wins::Overtaken::WaitsFor(_)
        ));
    }

    let after = s.updated_at(&olders).await;
    for d in &olders {
        assert!(
            after[d] > before[d],
            "a waiting deploy moves to the back of the sweep, so ten of them can't fill it"
        );
        assert_eq!(s.release_status(*d).await, "QUEUED");
    }
    Ok(())
}
