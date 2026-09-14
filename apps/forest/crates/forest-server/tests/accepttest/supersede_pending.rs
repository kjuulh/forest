//! The supersede-pending policy: collapsing a queue of pending releases to the
//! newest one. See `design/SKIP-TO-LATEST.md`.
//!
//! These drive `supersede::collapse_queue` — the single decision point the
//! scheduler calls — against a real database, with the queue seeded directly so
//! each test controls it exactly. The seeded rows deliberately name a
//! destination that was never registered: the scheduler runs process-wide in
//! this fixture, and a target it cannot resolve is one it leaves alone, so a
//! background sweep can neither drain the queue mid-assertion nor make the
//! outcome depend on timing. `collapse_queue` never reads the destination row —
//! it is handed the environment — so nothing under test is stubbed out.
//!
//! Deliberately few. The invariants — the newest is never superseded, no older
//! release wins, the plan is order-independent and idempotent, the branch guard
//! — are proved exhaustively by the pure tests in `services/supersede.rs`,
//! which need no database and no fixture. These five cover only what the pure
//! layer cannot: that the decision reaches the database as the right status and
//! reason, that a disabled policy writes nothing, that an in-flight deploy is
//! left alone, that concurrent evaluators agree, and that a collapsed pipeline
//! run ends SUPERSEDED rather than FAILED.
//!
//! The five run as one `#[tokio::test]`, in order, rather than as five. Each
//! `#[tokio::test]` builds its own runtime, and direct pool work binds
//! connections to it; when the runtime dies with the test those connections
//! are left broken in the pool the whole suite shares (sqlx logs *"a Tokio 1.x
//! context was found, but it is being shutdown"*). The rest of the suite
//! reaches the database through gRPC, so its work happens on the fixture's
//! long-lived runtime and it never notices. These do not, and as separate
//! tests they failed on `PoolTimedOut` during setup — in whichever one drew the
//! short straw, never on an assertion. One runtime for the module removes the
//! failure class outright. Each scenario keeps its own name, and a failure says
//! which one.

use forest_server::intent_coordinator;
use forest_server::services::policy::PolicyRegistryState;
use forest_server::services::release_event_store::ReleaseEventStoreState;
use forest_server::services::supersede::{self, Verdict};
use uuid::Uuid;

use crate::accepttest::fixtures::{Fixture, GivenReleaseFlow, fixture, testcase};
use crate::accepttest::release_flow::ReleaseFlowData;

/// A project, an environment, and a queue of pending releases against one
/// target key.
struct Seeded {
    fixture: Fixture,
    project_id: Uuid,
    destination_id: Uuid,
    environment: String,
    /// Queued releases, oldest first.
    releases: Vec<Uuid>,
    /// The in-flight release that keeps the background scheduler off this
    /// target — see `queue_releases`.
    sentinel: Uuid,
}

impl Seeded {
    async fn status(&self, release_id: Uuid) -> String {
        sqlx::query_scalar!(
            "SELECT status FROM release_states WHERE release_id = $1",
            release_id
        )
        .fetch_one(&self.fixture.db)
        .await
        .expect("release row")
    }

    async fn error_message(&self, release_id: Uuid) -> Option<String> {
        sqlx::query_scalar!(
            "SELECT error_message FROM release_states WHERE release_id = $1",
            release_id
        )
        .fetch_one(&self.fixture.db)
        .await
        .expect("release row")
    }

    async fn superseded_event_count(&self, release_id: Uuid) -> i64 {
        sqlx::query_scalar!(
            r#"SELECT count(*) as "count!"
               FROM release_events
               WHERE release_id = $1 AND event_type = 'release.superseded'"#,
            release_id
        )
        .fetch_one(&self.fixture.db)
        .await
        .expect("event count")
    }

    /// Run the decision for one release, exactly as the scheduler does.
    async fn collapse(&self, candidate: Uuid) -> Verdict {
        supersede::collapse_queue(
            &self.fixture.state.release_event_store(),
            &self.fixture.state.policy_registry(),
            &self.fixture.state.nats,
            candidate,
            self.project_id,
            self.destination_id,
            &self.environment,
        )
        .await
        .expect("collapse queue")
    }
}

/// One project for the whole module, built once.
///
/// Isolation between tests comes from the target key and the environment name,
/// not from a fresh project each time: every test takes a destination id of its
/// own and an environment of its own, and the policy is keyed on that
/// environment. Registering a user and uploading an artifact per test put
/// enough load on the shared fixture server to make registration itself fail,
/// which is a fixture problem being reported as a policy problem.
static PROJECT: tokio::sync::OnceCell<(Fixture, Uuid)> = tokio::sync::OnceCell::const_new();

async fn shared_project() -> anyhow::Result<(Fixture, Uuid)> {
    let (f, id) = PROJECT
        .get_or_try_init(|| async {
            let (given, _when, _then) = testcase::<ReleaseFlowData>().await?;

            let suffix = Uuid::now_v7();
            let org = format!("test-org-{suffix}");
            let env = format!("supersede-setup-{suffix}");
            let dest = format!("supersede-dest-{suffix}");

            given
                .a_registered_user()
                .await
                .an_organisation(&org)
                .await
                .an_environment(&env)
                .await
                .a_destination(&dest, &env)
                .await
                .an_uploaded_artifact()
                .await
                .an_annotated_release()
                .await;

            let f = fixture().await?;
            let project_id = sqlx::query_scalar!(
                "SELECT id FROM projects WHERE organisation = $1 AND project = 'test-project'",
                org,
            )
            .fetch_one(&f.db)
            .await?;

            Ok::<_, anyhow::Error>((f, project_id))
        })
        .await?;

    Ok((f.clone(), *id))
}

/// The shared project plus an environment name nothing else uses.
async fn a_project() -> anyhow::Result<(Fixture, Uuid, String)> {
    let (f, project_id) = shared_project().await?;
    Ok((f, project_id, format!("supersede-env-{}", Uuid::now_v7())))
}

/// Turn the supersede-pending policy on for `environment`. Named after the
/// environment, since policy names are unique per project and the project is
/// shared.
async fn enable_policy(f: &Fixture, project_id: Uuid, environment: &str, same_branch_only: bool) {
    sqlx::query!(
        r#"INSERT INTO policies (project_id, name, policy_type, config)
           VALUES ($1, $2, 'supersede_pending', $3)"#,
        project_id,
        format!("collapse-{environment}"),
        serde_json::json!({
            "target_environment": environment,
            "same_branch_only": same_branch_only,
            "cancel_in_progress": false,
        }),
    )
    .execute(&f.db)
    .await
    .expect("create supersede_pending policy");
}

/// `n` releases queued against one target, oldest first, each in its own
/// (non-pipeline) intent — the individual-deployment shape.
///
/// Returns the sentinel first, then the queued releases.
///
/// The sentinel is an `ASSIGNED` release for the same target, and it is what
/// keeps this test isolated from the fixture's background scheduler:
/// `pick_queued_releases` skips any target that already has something
/// `ASSIGNED` or `RUNNING`, so the sweep never touches these rows. Without it
/// the sweep retries every seeded release every five seconds forever — the rows
/// name a destination that does not exist, so they can never drain — and that
/// churn starves the ten-connection pool the whole suite shares. It is also the
/// honest setup: a queue only builds up behind a deploy that is already going.
///
/// `collapse_queue` reads only `QUEUED` rows, so the sentinel is invisible to
/// the code under test.
async fn queue_releases(
    f: &Fixture,
    project_id: Uuid,
    destination_id: Uuid,
    n: usize,
) -> (Uuid, Vec<Uuid>) {
    let sentinel = insert_release(f, project_id, destination_id, "ASSIGNED", -1).await;
    let mut ids = Vec::new();
    for i in 0..n {
        ids.push(insert_release(f, project_id, destination_id, "QUEUED", i as i64).await);
    }
    (sentinel, ids)
}

/// One release row for a target, `offset_secs` from now in the queue order.
async fn insert_release(
    f: &Fixture,
    project_id: Uuid,
    destination_id: Uuid,
    status: &str,
    offset_secs: i64,
) -> Uuid {
    let intent_id = Uuid::now_v7();
    let artifact = Uuid::now_v7();
    sqlx::query!(
        r#"INSERT INTO release_intents (id, artifact, annotation_id, project_id, status)
           VALUES ($1, $2, $3, $4, 'ACTIVE')"#,
        intent_id,
        artifact,
        Uuid::now_v7(),
        project_id,
    )
    .execute(&f.db)
    .await
    .expect("insert intent");

    let release_id = Uuid::now_v7();
    sqlx::query!(
        r#"INSERT INTO release_states (
               release_id, release_intent_id, project_id,
               destination_id, artifact_id, status, queued_at,
               assigned_at, last_heartbeat_at
           ) VALUES (
               $1, $2, $3, $4, $5, $6,
               now() + ($7 || ' seconds')::interval,
               CASE WHEN $6 = 'QUEUED' THEN NULL ELSE now() END,
               CASE WHEN $6 = 'QUEUED' THEN NULL ELSE now() END
           )"#,
        release_id,
        intent_id,
        project_id,
        destination_id,
        artifact,
        status,
        offset_secs.to_string(),
    )
    .execute(&f.db)
    .await
    .expect("insert release");

    release_id
}

async fn seeded(n: usize, policy: bool) -> anyhow::Result<Seeded> {
    let (f, project_id, environment) = a_project().await?;
    if policy {
        enable_policy(&f, project_id, &environment, false).await;
    }
    // Never registered: see the module comment.
    let destination_id = Uuid::now_v7();
    let (sentinel, releases) = queue_releases(&f, project_id, destination_id, n).await;

    Ok(Seeded {
        fixture: f,
        project_id,
        destination_id,
        environment,
        releases,
        sentinel,
    })
}

/// Leave nothing behind: a still-`QUEUED` row for a destination that was never
/// registered can never drain, and the scheduler's sweep would retry it every
/// five seconds for the life of the database.
async fn cleanup(s: &Seeded) {
    let _ = sqlx::query!(
        "UPDATE release_states
            SET status = 'CANCELLED', error_message = 'acceptance test cleanup',
                completed_at = now(), updated_at = now()
          WHERE project_id = $1 AND destination_id = $2
            AND status IN ('QUEUED', 'ASSIGNED', 'RUNNING')",
        s.project_id,
        s.destination_id,
    )
    .execute(&s.fixture.db)
    .await;
}

// ── The decision reaches the database ────────────────────────────────

/// The headline: a queue of five individual deployments for one target
/// collapses to one. The four it overtook end SUPERSEDED — not failed, not
/// deployed — each saying which release took its place, and the newest is
/// untouched and still dispatchable.
async fn a_queue_of_pending_deploys_collapses_to_the_newest() -> anyhow::Result<()> {
    let s = seeded(5, true).await?;
    let newest = *s.releases.last().unwrap();

    // The scheduler reaches the oldest first — that is the order its sweep
    // picks in.
    assert_eq!(s.collapse(s.releases[0]).await, Verdict::Superseded);

    for older in &s.releases[..4] {
        assert_eq!(
            s.status(*older).await,
            "SUPERSEDED",
            "every release the newest overtook is retired",
        );
        let reason = s.error_message(*older).await.unwrap_or_default();
        assert!(
            reason.contains("superseded by release") && reason.contains("policy 'collapse-"),
            "a superseded release must say which release took its place and \
             under which policy, got {reason:?}",
        );
        assert_eq!(
            s.superseded_event_count(*older).await,
            1,
            "exactly one release.superseded event, so the audit log reads once",
        );
    }

    assert_eq!(
        s.status(newest).await,
        "QUEUED",
        "the newest is never superseded",
    );
    assert_eq!(
        s.collapse(newest).await,
        Verdict::Proceed,
        "the newest proceeds to deploy",
    );

    cleanup(&s).await;
    Ok(())
}

/// With no policy configured, forest behaves exactly as it does today: every
/// queued release stays queued and every one of them deploys in turn.
async fn with_the_policy_off_nothing_is_superseded() -> anyhow::Result<()> {
    let s = seeded(4, false).await?;

    for r in &s.releases {
        assert_eq!(
            s.collapse(*r).await,
            Verdict::Proceed,
            "every release dispatches when the policy is off",
        );
    }

    for r in &s.releases {
        assert_eq!(s.status(*r).await, "QUEUED");
        assert_eq!(s.superseded_event_count(*r).await, 0);
    }

    cleanup(&s).await;
    Ok(())
}

// ── In-progress work is never interrupted ────────────────────────────

/// A deploy already in flight when newer releases arrive finishes. The queue
/// behind it still collapses, so the target runs two deploys — the one already
/// going, then the newest — rather than all of them.
async fn a_running_deploy_is_left_alone_and_the_queue_behind_it_collapses() -> anyhow::Result<()> {
    let s = seeded(3, true).await?;

    // The sentinel is the deploy already going; promote it to RUNNING, which is
    // where it would be once a runner picked it up.
    let in_flight = s.sentinel;
    sqlx::query!(
        "UPDATE release_states SET status = 'RUNNING', started_at = now()
         WHERE release_id = $1",
        in_flight,
    )
    .execute(&s.fixture.db)
    .await?;

    let newest = *s.releases.last().unwrap();
    s.collapse(s.releases[0]).await;

    assert_eq!(
        s.status(in_flight).await,
        "RUNNING",
        "an in-flight deploy is never interrupted: forest's destinations \
         cannot be cancelled into a defined state",
    );
    assert_eq!(s.superseded_event_count(in_flight).await, 0);
    assert_eq!(s.status(s.releases[0]).await, "SUPERSEDED");
    assert_eq!(s.status(s.releases[1]).await, "SUPERSEDED");
    assert_eq!(
        s.status(newest).await,
        "QUEUED",
        "the target runs the deploy already going, then the newest — two \
         deploys, not four",
    );

    cleanup(&s).await;
    Ok(())
}

// ── Race safety ──────────────────────────────────────────────────────

/// Concurrent evaluation cannot strand the newest and cannot supersede anything
/// twice. Two evaluators is enough to cover both interleavings that matter:
/// racing for the same row, and finding your own candidate already retired.
async fn concurrent_collapses_agree_on_exactly_one_survivor() -> anyhow::Result<()> {
    let s = seeded(3, true).await?;
    let newest = *s.releases.last().unwrap();

    let mut handles = Vec::new();
    for candidate in s.releases.clone() {
        let store = s.fixture.state.release_event_store();
        let policies = s.fixture.state.policy_registry();
        let nats = s.fixture.state.nats.clone();
        let (project_id, destination_id, environment) =
            (s.project_id, s.destination_id, s.environment.clone());

        handles.push(tokio::spawn(async move {
            supersede::collapse_queue(
                &store,
                &policies,
                &nats,
                candidate,
                project_id,
                destination_id,
                &environment,
            )
            .await
        }));
    }

    let mut proceeded = Vec::new();
    for (candidate, h) in s.releases.iter().zip(handles) {
        if h.await.expect("task")? == Verdict::Proceed {
            proceeded.push(*candidate);
        }
    }

    assert_eq!(
        proceeded,
        vec![newest],
        "exactly one release may proceed, and it must be the newest",
    );
    assert_eq!(s.status(newest).await, "QUEUED");

    for older in &s.releases[..2] {
        assert_eq!(s.status(*older).await, "SUPERSEDED");
        assert_eq!(
            s.superseded_event_count(*older).await,
            1,
            "a lost race is a no-op, not a second event",
        );
    }

    cleanup(&s).await;
    Ok(())
}

// ── Pipeline runs: the same mechanism, one level up ──────────────────

/// A queue of pipeline runs is the same queue: their deploy stages put
/// `release_states` rows against the same target key. Collapsing it must leave
/// the overtaken runs SUPERSEDED rather than FAILED — a collapsed queue that
/// pages whoever owns the project is worse than the queue.
async fn a_queue_of_pipeline_runs_collapses_and_the_overtaken_runs_are_not_failures()
-> anyhow::Result<()> {
    let (f, project_id, environment) = a_project().await?;
    enable_policy(&f, project_id, &environment, false).await;

    let destination_id = Uuid::now_v7();
    // Keeps the background scheduler off this target — see `queue_releases`.
    let sentinel = insert_release(&f, project_id, destination_id, "ASSIGNED", -1).await;

    // Three pipeline runs, each `deploy → verify`, each with its deploy stage
    // ACTIVE and one queued release against the shared destination.
    let stages = serde_json::json!({
        "deploy": { "type": "deploy", "environment": environment, "depends_on": [] },
        "verify": { "type": "wait", "duration_seconds": 60, "depends_on": ["deploy"] },
    });

    let mut runs = Vec::new();
    for i in 0..3usize {
        let intent_id = Uuid::now_v7();
        let artifact = Uuid::now_v7();
        let release_id = Uuid::now_v7();

        sqlx::query!(
            r#"INSERT INTO release_intents
                   (id, artifact, annotation_id, project_id, status, stages, stage_states)
               VALUES ($1, $2, $3, $4, 'ACTIVE', $5, $6)"#,
            intent_id,
            artifact,
            Uuid::now_v7(),
            project_id,
            stages,
            serde_json::json!({
                "deploy": {
                    "status": "ACTIVE",
                    "queued_at": "2026-01-01T00:00:00Z",
                    "started_at": "2026-01-01T00:00:00Z",
                    "release_ids": [release_id.to_string()],
                },
                "verify": { "status": "PENDING" },
            }),
        )
        .execute(&f.db)
        .await?;

        sqlx::query!(
            r#"INSERT INTO release_states (
                   release_id, release_intent_id, project_id, destination_id,
                   artifact_id, status, stage_id, queued_at
               ) VALUES ($1, $2, $3, $4, $5, 'QUEUED', 'deploy',
                         now() + ($6 || ' seconds')::interval)"#,
            release_id,
            intent_id,
            project_id,
            destination_id,
            artifact,
            i.to_string(),
        )
        .execute(&f.db)
        .await?;

        runs.push((intent_id, release_id));
    }

    let s = Seeded {
        fixture: f.clone(),
        project_id,
        destination_id,
        environment,
        releases: runs.iter().map(|(_, r)| *r).collect(),
        sentinel,
    };

    s.collapse(runs[0].1).await;

    assert_eq!(s.status(runs[0].1).await, "SUPERSEDED");
    assert_eq!(s.status(runs[1].1).await, "SUPERSEDED");
    assert_eq!(
        s.status(runs[2].1).await,
        "QUEUED",
        "the newest pipeline run still deploys",
    );

    // Drive the coordinator, as the NATS nudge does in production.
    for (intent_id, _) in &runs {
        intent_coordinator::evaluate(&f.state, *intent_id).await?;
    }

    for (intent_id, _) in &runs[..2] {
        let row = sqlx::query!(
            r#"SELECT status, stage_states FROM release_intents WHERE id = $1"#,
            intent_id,
        )
        .fetch_one(&f.db)
        .await?;
        let stage_states = row.stage_states.unwrap_or_default();

        assert_eq!(
            row.status, "SUPERSEDED",
            "an overtaken pipeline run is superseded, not failed",
        );
        assert_eq!(stage_states["deploy"]["status"], "SUPERSEDED");
        assert_eq!(
            stage_states["verify"]["status"], "SUPERSEDED",
            "the rest of the run is moot, and inherits SUPERSEDED rather than \
             CANCELLED — nobody cancelled it",
        );
    }

    let newest_status = sqlx::query_scalar!(
        "SELECT status FROM release_intents WHERE id = $1",
        runs[2].0
    )
    .fetch_one(&f.db)
    .await?;
    assert_eq!(
        newest_status, "ACTIVE",
        "the newest run carries on with its deploy still queued",
    );

    cleanup(&s).await;
    Ok(())
}

// ── Driver ───────────────────────────────────────────────────────────

/// The scenarios above, in order, on one runtime. See the module comment for
/// why they are not five separate `#[tokio::test]`s.
#[tokio::test(flavor = "multi_thread")]
async fn supersede_pending_policy() -> anyhow::Result<()> {
    use anyhow::Context;

    a_queue_of_pending_deploys_collapses_to_the_newest()
        .await
        .context("a queue of pending deploys collapses to the newest")?;

    with_the_policy_off_nothing_is_superseded()
        .await
        .context("with the policy off, nothing is superseded")?;

    a_running_deploy_is_left_alone_and_the_queue_behind_it_collapses()
        .await
        .context("a running deploy is left alone and the queue behind it collapses")?;

    concurrent_collapses_agree_on_exactly_one_survivor()
        .await
        .context("concurrent collapses agree on exactly one survivor")?;

    a_queue_of_pipeline_runs_collapses_and_the_overtaken_runs_are_not_failures()
        .await
        .context("a queue of pipeline runs collapses, and the overtaken runs are not failures")?;

    Ok(())
}
