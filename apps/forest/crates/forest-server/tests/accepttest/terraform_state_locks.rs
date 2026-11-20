use axum::body::{Body, to_bytes};
use base64::Engine as _;
use forest_server::destinations::terraform_locks::{
    Holder, StaleLockCheck, in_process_principal, runner_principal,
};
use forest_server::destinations::terraformv1::{TerraformStateStoreState, TerraformV1Server};
use forest_server::services::release_registry::ReleaseItem;
use http::{Method, Request, StatusCode};
use tower::ServiceExt;
use uuid::Uuid;

use super::fixtures::{Fixture, fixture};

fn state_id() -> String {
    format!("test.locks.{}", Uuid::new_v4())
}

fn destination() -> forest_models::Destination {
    forest_models::Destination {
        organisation: "understory".into(),
        name: "dev/eu-west-1/ecs".into(),
        environment: "dev".into(),
        metadata: Default::default(),
        sensitive_keys: vec![],
        destination_type: forest_models::DestinationType {
            organisation: "forest".into(),
            name: "terraform".into(),
            version: 1,
            description: String::new(),
            fields: vec![],
        },
    }
}

fn lock_body(lock_id: &str) -> String {
    serde_json::json!({
        "ID": lock_id,
        "Operation": "OperationTypeApply",
        "Info": "",
        "Who": "root@ip-10-0-0-1",
        "Version": "1.12.6",
        "Created": "2026-11-19T10:00:00.123456789Z",
        "Path": "",
    })
    .to_string()
}

async fn lock(f: &Fixture, state_id: &str, password: &str, body: &str) -> (StatusCode, String) {
    let auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("forest-terraform-v1:{password}"))
    );
    let response = TerraformV1Server::router(f.state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{state_id}/lock"))
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(http::header::AUTHORIZATION, auth)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn lock_as_release(f: &Fixture, state_id: &str, release_id: Uuid, lock_id: &str) {
    let password =
        f.state
            .terraform_state_store()
            .in_process_credential(state_id, &destination(), release_id);
    let (status, body) = lock(f, state_id, &password, &lock_body(lock_id)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn age_lock(f: &Fixture, state_id: &str, minutes: i32) {
    sqlx::query(
        "UPDATE terraform_state_locks
            SET acquired_at = now() - make_interval(mins => $2)
          WHERE state_id = $1",
    )
    .bind(state_id)
    .bind(minutes)
    .execute(&f.db)
    .await
    .unwrap();
}

async fn lock_id(f: &Fixture, state_id: &str) -> Option<String> {
    sqlx::query_scalar("SELECT lock_id FROM terraform_state_locks WHERE state_id = $1")
        .bind(state_id)
        .fetch_optional(&f.db)
        .await
        .unwrap()
}

struct Release {
    item: ReleaseItem,
}

impl Release {
    async fn insert(f: &Fixture, status: &str) -> Self {
        Self::insert_on(f, status, Uuid::new_v4(), Uuid::new_v4()).await
    }

    async fn insert_on(f: &Fixture, status: &str, project_id: Uuid, destination_id: Uuid) -> Self {
        let intent_id = Uuid::now_v7();
        let artifact_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO release_intents (id, artifact, annotation_id, project_id, status)
             VALUES ($1, $2, $3, $4, 'ACTIVE')",
        )
        .bind(intent_id)
        .bind(artifact_id)
        .bind(Uuid::new_v4())
        .bind(project_id)
        .execute(&f.db)
        .await
        .unwrap();

        let release_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO release_states (
                 release_id, release_intent_id, project_id, destination_id, artifact_id, status,
                 runner_id, assigned_at, started_at, last_heartbeat_at
             ) VALUES ($1, $2, $3, $4, $5, $6, 'in-process', now(), now(), now())",
        )
        .bind(release_id)
        .bind(intent_id)
        .bind(project_id)
        .bind(destination_id)
        .bind(artifact_id)
        .bind(status)
        .execute(&f.db)
        .await
        .unwrap();

        Self {
            item: ReleaseItem {
                id: release_id,
                release_intent_id: intent_id,
                artifact: artifact_id,
                project_id,
                destination_id,
                status: status.into(),
                project: "test-project".into(),
            },
        }
    }

    fn id(&self) -> Uuid {
        self.item.id
    }

    async fn status(&self, f: &Fixture) -> String {
        sqlx::query_scalar("SELECT status FROM release_states WHERE release_id = $1")
            .bind(self.id())
            .fetch_one(&f.db)
            .await
            .unwrap()
    }

    async fn set(&self, f: &Fixture, sql: &'static str) {
        sqlx::query(sql)
            .bind(self.id())
            .execute(&f.db)
            .await
            .unwrap();
    }

    async fn finish(&self, f: &Fixture) {
        self.set(
            f,
            "UPDATE release_states SET status = 'CANCELLED', error_message = 'acceptance test cleanup', completed_at = now() WHERE release_id = $1",
        )
        .await;
    }
}

async fn reap(f: &Fixture) {
    forest_server::release_reaper::ReleaseReaper::new(
        &f.state,
        forest_server::runner_manager::RunnerManager::new(),
    )
    .reap()
    .await
    .unwrap();
}

async fn insert_lock(
    f: &Fixture,
    state_id: &str,
    lock_id: &str,
    principal: Option<String>,
    age_minutes: i32,
) {
    let holder = Holder::parse(principal.as_deref());
    sqlx::query(
        "INSERT INTO terraform_state_locks
             (state_id, lock_id, operation, who, principal, release_id, replica, acquired_at)
         VALUES ($1, $2, 'OperationTypeApply', 'root@task', $3, $4, $5,
                 now() - make_interval(mins => $6))",
    )
    .bind(state_id)
    .bind(lock_id)
    .bind(&principal)
    .bind(holder.release_id())
    .bind(holder.replica())
    .bind(age_minutes)
    .execute(&f.db)
    .await
    .unwrap();
}

async fn replica_seen(f: &Fixture, name: &str, seconds_ago: i32) {
    sqlx::query(
        "INSERT INTO forest_replicas (name, started_at, last_seen_at)
         VALUES ($1, now() - interval '1 hour', now() - make_interval(secs => $2))
         ON CONFLICT (name) DO UPDATE SET last_seen_at = EXCLUDED.last_seen_at",
    )
    .bind(name)
    .bind(f64::from(seconds_ago))
    .execute(&f.db)
    .await
    .unwrap();
}

fn replica(label: &str) -> String {
    format!("test-{label}#{}", &Uuid::new_v4().simple().to_string()[..8])
}

async fn check(f: &Fixture, state_id: &str, release: &Release) -> StaleLockCheck {
    f.state
        .terraform_state_store()
        .check_stale_lock(state_id, &release.item)
        .await
        .unwrap()
}

async fn unlock(f: &Fixture, state_id: &str) {
    f.state
        .terraform_state_store()
        .force_unlock(state_id)
        .await
        .unwrap();
}

fn kept_reason(check: StaleLockCheck) -> String {
    match check {
        StaleLockCheck::Kept { reason, .. } => reason,
        other => panic!("the lock must be kept: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lock_records_what_terraform_sent_and_the_release_and_replica_that_took_it() {
    let f = fixture().await.unwrap();
    let id = state_id();
    let owner = Release::insert(&f, "RUNNING").await;

    lock_as_release(&f, &id, owner.id(), "c7081117-held").await;

    let held = f
        .state
        .terraform_state_store()
        .current_lock(&id)
        .await
        .unwrap()
        .expect("the state is locked");
    let this_replica = forest_server::replicas::replica_name();
    assert_eq!(held.lock_id, "c7081117-held");
    assert_eq!(held.operation, "OperationTypeApply");
    assert_eq!(held.who, "root@ip-10-0-0-1");
    assert_eq!(held.tf_version, "1.12.6");
    assert_eq!(held.tf_created, "2026-11-19T10:00:00.123456789Z");
    assert_eq!(held.release_id, Some(owner.id()));
    assert_eq!(held.replica.as_deref(), Some(this_replica));
    assert_eq!(
        held.principal,
        Some(in_process_principal(
            owner.id(),
            this_replica,
            &destination()
        ))
    );

    let next = Release::insert(&f, "RUNNING").await;
    let password = f
        .state
        .terraform_state_store()
        .runner_credential(&id, next.id(), "runner-7");
    let (status, body) = lock(&f, &id, &password, &lock_body("next-run")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let conflict: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(conflict["ID"], "c7081117-held");
    assert_eq!(conflict["Operation"], "OperationTypeApply");
    let who = conflict["Who"].as_str().unwrap();
    assert_eq!(
        who,
        format!(
            "forest release {} on {this_replica} (root@ip-10-0-0-1)",
            owner.id()
        ),
        "the conflict terraform prints names the release and replica holding the lock"
    );
    assert!(
        chrono::DateTime::parse_from_rfc3339(conflict["Created"].as_str().unwrap()).is_ok(),
        "{conflict}"
    );

    unlock(&f, &id).await;
    owner.finish(&f).await;
    next.finish(&f).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_reaper_fails_releases_but_leaves_their_locks_alone() {
    let f = fixture().await.unwrap();
    let lost = Release::insert(&f, "RUNNING").await;
    let stuck = Release::insert(&f, "RUNNING").await;
    let lost_state = state_id();
    let stuck_state = state_id();
    lock_as_release(&f, &lost_state, lost.id(), "lost-run").await;
    lock_as_release(&f, &stuck_state, stuck.id(), "stuck-run").await;
    lost.set(
        &f,
        "UPDATE release_states SET last_heartbeat_at = now() - interval '10 minutes' WHERE release_id = $1",
    )
    .await;
    stuck
        .set(
            &f,
            "UPDATE release_states SET started_at = now() - interval '2 hours', last_heartbeat_at = NULL WHERE release_id = $1",
        )
        .await;

    reap(&f).await;

    assert_eq!(lost.status(&f).await, "FAILED");
    assert_eq!(stuck.status(&f).await, "TIMED_OUT");
    assert_eq!(
        lock_id(&f, &lost_state).await.as_deref(),
        Some("lost-run"),
        "a lost heartbeat does not prove terraform stopped, so its lock stays"
    );
    assert_eq!(
        lock_id(&f, &stuck_state).await.as_deref(),
        Some("stuck-run"),
        "a timed-out release may still be applying, so its lock stays"
    );

    unlock(&f, &lost_state).await;
    unlock(&f, &stuck_state).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_releases_lock_is_kept_while_its_replica_is_alive() {
    let f = fixture().await.unwrap();
    let failed = Release::insert(&f, "FAILED").await;
    let next = Release::insert(&f, "RUNNING").await;
    let live = replica("live");
    replica_seen(&f, &live, 10).await;
    let id = state_id();
    insert_lock(
        &f,
        &id,
        "long-apply",
        Some(in_process_principal(failed.id(), &live, &destination())),
        240,
    )
    .await;

    assert_eq!(
        kept_reason(check(&f, &id, &next).await),
        format!(
            "locked by release {} on replica {live}, still alive",
            failed.id()
        )
    );
    assert_eq!(lock_id(&f, &id).await.as_deref(), Some("long-apply"));

    unlock(&f, &id).await;
    next.finish(&f).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_releases_lock_is_released_once_its_replica_stopped_or_is_long_gone() {
    let f = fixture().await.unwrap();
    let next = Release::insert(&f, "RUNNING").await;

    let stopped = replica("stopped");
    replica_seen(&f, &stopped, 5).await;
    forest_server::replicas::deregister(&f.db, &stopped)
        .await
        .unwrap();
    let long_gone = replica("long-gone");
    replica_seen(&f, &long_gone, 1500).await;
    let never_seen = replica("never-seen");

    for (status, replica) in [
        ("FAILED", &stopped),
        ("FAILED", &long_gone),
        ("TIMED_OUT", &never_seen),
    ] {
        let owner = Release::insert(&f, status).await;
        let id = state_id();
        insert_lock(
            &f,
            &id,
            "dead-run",
            Some(in_process_principal(owner.id(), replica, &destination())),
            1,
        )
        .await;

        let result = check(&f, &id, &next).await;
        assert!(
            matches!(&result, StaleLockCheck::Released { lock, .. } if lock.lock_id == "dead-run"),
            "{status} on {replica}: {result:?}"
        );
        assert_eq!(lock_id(&f, &id).await, None);
    }
    next.finish(&f).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_releases_lock_is_kept_while_its_replica_is_only_silent() {
    let f = fixture().await.unwrap();
    let failed = Release::insert(&f, "FAILED").await;
    let next = Release::insert(&f, "RUNNING").await;
    let stalled = replica("stalled");
    replica_seen(&f, &stalled, 600).await;
    let id = state_id();
    insert_lock(
        &f,
        &id,
        "stalled-apply",
        Some(in_process_principal(failed.id(), &stalled, &destination())),
        30,
    )
    .await;

    let reason = kept_reason(check(&f, &id, &next).await);
    assert!(
        reason.starts_with(&format!(
            "locked by release {} on replica {stalled}, not heard from since ",
            failed.id()
        )),
        "a replica silent for 10 minutes may be stalled mid-apply: {reason}"
    );
    assert_eq!(lock_id(&f, &id).await.as_deref(), Some("stalled-apply"));

    unlock(&f, &id).await;
    next.finish(&f).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_releases_lock_is_kept_even_when_its_replica_looks_gone() {
    let f = fixture().await.unwrap();
    let running = Release::insert(&f, "RUNNING").await;
    let next = Release::insert(&f, "RUNNING").await;
    let gone = replica("gone");
    replica_seen(&f, &gone, 600).await;
    let id = state_id();
    insert_lock(
        &f,
        &id,
        "running",
        Some(in_process_principal(running.id(), &gone, &destination())),
        120,
    )
    .await;

    assert_eq!(
        kept_reason(check(&f, &id, &next).await),
        format!(
            "locked by release {} on replica {gone}, still running",
            running.id()
        )
    );
    assert_eq!(lock_id(&f, &id).await.as_deref(), Some("running"));

    unlock(&f, &id).await;
    running.finish(&f).await;
    next.finish(&f).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_humans_lock_is_never_released_however_old() {
    let f = fixture().await.unwrap();
    let next = Release::insert(&f, "RUNNING").await;
    for principal in [
        format!("user:{}", Uuid::new_v4()),
        format!("app:{}", Uuid::new_v4()),
        format!("service_account:{}", Uuid::new_v4()),
    ] {
        let id = state_id();
        insert_lock(&f, &id, "operator", Some(principal.clone()), 600).await;

        let reason = kept_reason(check(&f, &id, &next).await);
        assert!(
            reason.starts_with(&format!("locked by {principal} since "))
                && reason.contains("forest state unlock"),
            "{reason}"
        );
        assert_eq!(lock_id(&f, &id).await.as_deref(), Some("operator"));
        unlock(&f, &id).await;
    }
    next.finish(&f).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remote_runners_lock_is_never_released_however_old() {
    let f = fixture().await.unwrap();
    let owner = Release::insert(&f, "FAILED").await;
    let next = Release::insert(&f, "RUNNING").await;
    let id = state_id();
    insert_lock(
        &f,
        &id,
        "remote",
        Some(runner_principal(owner.id(), "runner-7")),
        600,
    )
    .await;

    let reason = kept_reason(check(&f, &id, &next).await);
    assert!(
        reason.contains(&format!(
            "forest runner runner-7 for release {}",
            owner.id()
        )) && reason.contains("600m ago"),
        "{reason}"
    );
    assert_eq!(lock_id(&f, &id).await.as_deref(), Some("remote"));

    unlock(&f, &id).await;
    next.finish(&f).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_lock_is_released_only_after_an_hour_and_when_the_destination_is_idle() {
    let f = fixture().await.unwrap();
    let destination_id = Uuid::new_v4();
    let next = Release::insert_on(&f, "ASSIGNED", Uuid::new_v4(), destination_id).await;
    let id = state_id();
    insert_lock(&f, &id, "legacy", None, 30).await;

    kept_reason(check(&f, &id, &next).await);
    assert_eq!(lock_id(&f, &id).await.as_deref(), Some("legacy"));

    age_lock(&f, &id, 90).await;
    let other = Release::insert_on(&f, "RUNNING", Uuid::new_v4(), destination_id).await;
    let busy = kept_reason(check(&f, &id, &next).await);
    other.finish(&f).await;
    assert!(busy.contains("another release"), "{busy}");
    assert_eq!(lock_id(&f, &id).await.as_deref(), Some("legacy"));

    let idle = check(&f, &id, &next).await;
    assert!(matches!(idle, StaleLockCheck::Released { .. }), "{idle:?}");
    assert_eq!(lock_id(&f, &id).await, None);
    next.finish(&f).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_release_never_breaks_its_own_lock() {
    let f = fixture().await.unwrap();
    let me = Release::insert(&f, "RUNNING").await;
    let id = state_id();
    lock_as_release(&f, &id, me.id(), "mine").await;
    age_lock(&f, &id, 120).await;

    assert_eq!(check(&f, &id, &me).await, StaleLockCheck::HeldByThisRelease);
    assert_eq!(lock_id(&f, &id).await.as_deref(), Some("mine"));

    unlock(&f, &id).await;
    me.finish(&f).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replica_is_alive_while_it_beats_then_silent_then_gone() {
    use forest_server::replicas::{DEFAULT_GONE_AFTER, Liveness, beat, liveness};

    let f = fixture().await.unwrap();
    let name = replica("beating");

    assert_eq!(
        liveness(&f.db, &name, DEFAULT_GONE_AFTER).await.unwrap(),
        Liveness::Gone,
        "a replica that never beat is gone"
    );
    beat(&f.db, &name).await.unwrap();
    assert_eq!(
        liveness(&f.db, &name, DEFAULT_GONE_AFTER).await.unwrap(),
        Liveness::Alive
    );
    replica_seen(&f, &name, 600).await;
    assert!(matches!(
        liveness(&f.db, &name, DEFAULT_GONE_AFTER).await.unwrap(),
        Liveness::Silent { .. }
    ));
    replica_seen(&f, &name, 1500).await;
    assert_eq!(
        liveness(&f.db, &name, DEFAULT_GONE_AFTER).await.unwrap(),
        Liveness::Gone
    );
}

async fn run_heartbeat_through_shutdown(
    f: &Fixture,
    name: &str,
    drain: forest_server::release_drain::ReleaseDrain,
) {
    use notmad::Component as _;

    let heartbeat = forest_server::replicas::ReplicaHeartbeat::named(
        f.db.clone(),
        drain.clone(),
        name.to_string(),
    );
    heartbeat.setup().await.unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let running = tokio::spawn({
        let cancel = cancel.clone();
        async move { heartbeat.run(cancel).await }
    });
    drain.drain().await;
    cancel.cancel();
    running.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replica_that_drained_every_release_marks_itself_stopped() {
    use forest_server::replicas::{DEFAULT_GONE_AFTER, Liveness, liveness};
    use std::time::Duration;

    let f = fixture().await.unwrap();
    let clean = replica("clean");
    run_heartbeat_through_shutdown(
        &f,
        &clean,
        forest_server::release_drain::ReleaseDrain::with_graces(
            Duration::from_millis(100),
            Duration::from_millis(100),
            Duration::from_millis(100),
        ),
    )
    .await;
    assert_eq!(
        liveness(&f.db, &clean, DEFAULT_GONE_AFTER).await.unwrap(),
        Liveness::Stopped
    );

    let stuck = replica("stuck");
    let drain = forest_server::release_drain::ReleaseDrain::with_graces(
        Duration::from_millis(100),
        Duration::from_millis(100),
        Duration::from_millis(100),
    );
    drain.spawn(std::future::pending());
    run_heartbeat_through_shutdown(&f, &stuck, drain).await;
    assert_eq!(
        liveness(&f.db, &stuck, DEFAULT_GONE_AFTER).await.unwrap(),
        Liveness::Alive,
        "a replica that could not stop a release does not claim to be stopped"
    );
}
