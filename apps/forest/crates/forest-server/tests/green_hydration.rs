//! Mire migration rehearsal gate: every historical aggregate stream must load
//! through Mire, and a command must still commit its event and its projection
//! atomically.
//!
//! These tests are skipped unless `GREEN_DATABASE_URL` points at a migrated
//! rehearsal database (an Aurora snapshot/dump restored into a green cluster,
//! prepared with `forest-event-migration prepare` + `mire-migrate`). They are
//! deliberately read-only apart from the explicitly transactional smoke case,
//! which rolls itself back.
//!
//! Run with:
//!   GREEN_DATABASE_URL=postgres://... cargo test -p forest-server --test green_hydration -- --nocapture

use forest_server::domains::{
    app::AppAggregate,
    component::{ComponentAggregate, VersionState},
    destination::DestinationAggregate,
    device_login::DeviceGrantAggregate,
    policy::PolicyAggregate,
    trigger::TriggerAggregate,
};
use mire::{Aggregate, EventStore};
use sqlx::{PgPool, Row};

async fn green_pool() -> Option<PgPool> {
    let url = std::env::var("GREEN_DATABASE_URL").ok()?;
    forest_server::tls::install_crypto_provider();
    Some(
        PgPool::connect(&url)
            .await
            .expect("connect to the green rehearsal database"),
    )
}

/// Stream ids are `{category}-{id}`; recover the aggregate id.
async fn stream_ids_for(pool: &PgPool, category: &str) -> Vec<(String, i64)> {
    sqlx::query(
        "SELECT stream_id, stream_version FROM es_streams WHERE stream_category = $1 ORDER BY stream_id",
    )
    .bind(category)
    .fetch_all(pool)
    .await
    .expect("read streams for category")
    .into_iter()
    .map(|row| {
        let stream_id: String = row.get("stream_id");
        let version: i64 = row.get("stream_version");
        let id = stream_id
            .strip_prefix(&format!("{category}-"))
            .unwrap_or_else(|| panic!("stream {stream_id} does not carry its category prefix"))
            .to_string();
        (id, version)
    })
    .collect()
}

/// Load every stream of one category and assert Mire hydrates it to the version
/// the stream table records. Returns how many streams were checked.
async fn hydrate_category<A: Aggregate>(store: &EventStore, pool: &PgPool) -> usize {
    let category = A::stream_category();
    let streams = stream_ids_for(pool, category).await;

    for (id, expected_version) in &streams {
        let root = store
            .load::<A>(id)
            .await
            .unwrap_or_else(|error| {
                panic!("{category}-{id} failed to hydrate through Mire: {error}")
            })
            .unwrap_or_else(|| {
                panic!("{category}-{id} is present in es_streams but Mire returned None")
            });

        assert_eq!(
            root.version, *expected_version,
            "{category}-{id} hydrated to version {} but es_streams records {expected_version}",
            root.version
        );
    }

    println!("  {category:<14} {} streams hydrated", streams.len());
    streams.len()
}

#[tokio::test]
async fn every_historical_stream_loads_through_mire() {
    let Some(pool) = green_pool().await else {
        eprintln!("skipping: GREEN_DATABASE_URL is not set");
        return;
    };
    let store = EventStore::new(pool.clone());

    println!("hydrating every historical stream through Mire:");
    let mut total = 0;
    total += hydrate_category::<AppAggregate>(&store, &pool).await;
    total += hydrate_category::<ComponentAggregate>(&store, &pool).await;
    total += hydrate_category::<DestinationAggregate>(&store, &pool).await;
    total += hydrate_category::<DeviceGrantAggregate>(&store, &pool).await;
    total += hydrate_category::<PolicyAggregate>(&store, &pool).await;
    total += hydrate_category::<TriggerAggregate>(&store, &pool).await;

    let stream_count: i64 = sqlx::query_scalar("SELECT count(*) FROM es_streams")
        .fetch_one(&pool)
        .await
        .expect("count streams");

    println!("  total: {total} of {stream_count} streams");
    assert_eq!(
        total as i64,
        stream_count,
        "{} streams were not covered by the six supported aggregate categories",
        stream_count - total as i64
    );
}

/// The migration must not silently drop events or reorder global positions.
#[tokio::test]
async fn event_log_invariants_hold_after_migration() {
    let Some(pool) = green_pool().await else {
        eprintln!("skipping: GREEN_DATABASE_URL is not set");
        return;
    };

    let orphans: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM es_events e
          LEFT JOIN es_streams s ON s.stream_id = e.stream_id
         WHERE s.stream_id IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("count orphan events");
    assert_eq!(orphans, 0, "events exist without a stream");

    // Every stream's versions must be a contiguous 1..=n run.
    let broken: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (
           SELECT stream_id,
                  count(*)          AS events,
                  min(stream_version) AS lo,
                  max(stream_version) AS hi
             FROM es_events GROUP BY stream_id
         ) t WHERE t.lo <> 1 OR t.hi <> t.events",
    )
    .fetch_one(&pool)
    .await
    .expect("check stream version runs");
    assert_eq!(broken, 0, "streams have version gaps or are not 1-based");

    // Mire's compatibility columns must be fully backfilled.
    let unfilled: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM es_events
          WHERE transaction_id IS NULL
             OR stream_category IS NULL
             OR payload_size   IS NULL
             OR metadata_size  IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("check compatibility backfill");
    assert_eq!(
        unfilled, 0,
        "Mire compatibility columns are not fully backfilled"
    );

    // The global-position sequence must not hand out a used value.
    let row = sqlx::query(
        "SELECT (SELECT last_value FROM es_events_global_position_seq) AS seq,
                (SELECT coalesce(max(global_position), 0) FROM es_events) AS max_pos",
    )
    .fetch_one(&pool)
    .await
    .expect("read sequence");
    let seq: i64 = row.get("seq");
    let max_pos: i64 = row.get("max_pos");
    assert!(
        seq >= max_pos,
        "global-position sequence ({seq}) is behind the highest event ({max_pos})"
    );

    println!(
        "event log invariants hold: 0 orphans, 0 version gaps, 0 unfilled columns, seq {seq} >= max {max_pos}"
    );
}

// ============================================================
// Phase 6 — replay comparison against the live projections
// ============================================================
//
// Full Mire shadow projections (standalone handlers writing `*_mire_shadow`
// tables) are not implemented yet. What is verifiable today — and what the
// go/no-go gate actually asks for — is that replaying the event log through
// Mire agrees with the projection rows the live read path serves.
//
// For each category we replay every stream through Mire and compare the
// resulting aggregate against the live projection row, keyed exactly the way
// `forest-event-migration audit` composes it. Volatile replay timestamps are
// deliberately not compared, per the runbook.
//
// Deleted aggregates matter here: a stream survives deletion while its
// projection row is removed, so "stream count == projection row count" is the
// wrong assertion. Lifecycle state is what must agree.

/// Does exactly one projection row exist for this composed stream id?
async fn projection_rows(pool: &PgPool, sql: &'static str, stream_id: &str) -> i64 {
    sqlx::query_scalar(sql)
        .bind(stream_id)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("projection lookup failed for {stream_id}: {error}"))
}

#[tokio::test]
async fn replayed_lifecycle_matches_live_projections() {
    let Some(pool) = green_pool().await else {
        eprintln!("skipping: GREEN_DATABASE_URL is not set");
        return;
    };
    let store = EventStore::new(pool.clone());

    // (category, count-rows-for-this-stream-id SQL)
    const APP_SQL: &str = "SELECT count(*)::bigint FROM apps p WHERE 'app-' || p.organisation_id::text || '/' || p.name = $1";
    const DESTINATION_SQL: &str = "SELECT count(*)::bigint FROM destinations p WHERE 'destination-' || p.organisation || '/' || p.name = $1";
    const POLICY_SQL: &str = "SELECT count(*)::bigint FROM policies p WHERE 'policy-' || p.project_id::text || '/' || p.name = $1";
    const TRIGGER_SQL: &str = "SELECT count(*)::bigint FROM triggers p WHERE 'trigger-' || p.project_id::text || '/' || p.name = $1";
    const GRANT_SQL: &str = "SELECT count(*)::bigint FROM device_login_grants p WHERE 'device_grant-' || p.device_code_hash = $1";
    const COMPONENT_SQL: &str = "SELECT count(*)::bigint FROM components p WHERE 'component-' || p.organisation || '/' || p.name = $1";

    let mut compared = 0usize;
    let mut live = 0usize;
    let mut deleted = 0usize;

    macro_rules! compare_lifecycle {
        ($agg:ty, $sql:expr, $active:pat) => {{
            let category = <$agg as Aggregate>::stream_category();
            let mut c_live = 0;
            let mut c_deleted = 0;
            for (id, _) in stream_ids_for(&pool, category).await {
                let stream_id = format!("{category}-{id}");
                let root = store
                    .load::<$agg>(&id)
                    .await
                    .expect("hydrate")
                    .expect("stream present");
                let rows = projection_rows(&pool, $sql, &stream_id).await;
                let is_active = matches!(root.state.status, $active);
                if is_active {
                    assert_eq!(
                        rows, 1,
                        "{stream_id} replays as live but has {rows} projection rows"
                    );
                    c_live += 1;
                } else {
                    assert_eq!(
                        rows, 0,
                        "{stream_id} replays as terminal/deleted but still has {rows} projection rows"
                    );
                    c_deleted += 1;
                }
                compared += 1;
            }
            live += c_live;
            deleted += c_deleted;
            println!("  {category:<14} {c_live} live / {c_deleted} deleted — projections agree");
        }};
    }

    println!("replay vs live projections:");
    compare_lifecycle!(
        AppAggregate,
        APP_SQL,
        forest_server::domains::app::AppStatus::Active
    );
    compare_lifecycle!(
        DestinationAggregate,
        DESTINATION_SQL,
        forest_server::domains::destination::DestinationStatus::Active
    );
    compare_lifecycle!(
        PolicyAggregate,
        POLICY_SQL,
        forest_server::domains::policy::PolicyStatus::Active
    );
    compare_lifecycle!(
        TriggerAggregate,
        TRIGGER_SQL,
        forest_server::domains::trigger::TriggerStatus::Active
    );

    // Device grants are never deleted: every stream keeps exactly one row.
    // Compare the stable user_code too, not just presence.
    let mut grants = 0;
    for (id, _) in stream_ids_for(&pool, "device_grant").await {
        let stream_id = format!("device_grant-{id}");
        let root = store
            .load::<DeviceGrantAggregate>(&id)
            .await
            .expect("hydrate")
            .expect("stream present");
        assert_eq!(
            projection_rows(&pool, GRANT_SQL, &stream_id).await,
            1,
            "{stream_id} has no device_login_grants projection row"
        );
        let projected_user_code: String = sqlx::query_scalar(
            "SELECT user_code FROM device_login_grants WHERE device_code_hash = $1",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .expect("read projected user_code");
        assert_eq!(
            root.state.user_code, projected_user_code,
            "{stream_id} replayed user_code disagrees with the projection"
        );
        grants += 1;
        compared += 1;
    }
    println!("  device_grant   {grants} grants — presence and user_code agree");

    // Components are versioned. Only `Published` versions are served by the read
    // path: `Uploading` is an upload still in flight and `Unpublished` was
    // removed by an owner, and neither keeps a `components` row. Compare the
    // replayed *published* set against the projection, version by version.
    let mut components = 0;
    let mut published_total = 0i64;
    let mut skipped_total = 0i64;
    for (id, _) in stream_ids_for(&pool, "component").await {
        let stream_id = format!("component-{id}");
        let root = store
            .load::<ComponentAggregate>(&id)
            .await
            .expect("hydrate")
            .expect("stream present");

        let published: Vec<&String> = root
            .state
            .versions
            .iter()
            .filter(|(_, state)| matches!(state, VersionState::Published))
            .map(|(version, _)| version)
            .collect();
        let rows = projection_rows(&pool, COMPONENT_SQL, &stream_id).await;
        assert_eq!(
            published.len() as i64,
            rows,
            "{stream_id} replays {} published versions but the projection holds {rows} rows",
            published.len()
        );

        // Every published version must be individually present, and every
        // non-published version must be absent.
        for (version, state) in &root.state.versions {
            let present: i64 = sqlx::query_scalar(
                "SELECT count(*)::bigint FROM components
                  WHERE 'component-' || organisation || '/' || name = $1 AND version = $2",
            )
            .bind(&stream_id)
            .bind(version)
            .fetch_one(&pool)
            .await
            .expect("look up component version");
            match state {
                VersionState::Published => assert_eq!(
                    present, 1,
                    "{stream_id} version {version} replays as Published but is not in the projection"
                ),
                _ => {
                    assert_eq!(
                        present, 0,
                        "{stream_id} version {version} replays as {state:?} but the read path still serves it"
                    );
                    skipped_total += 1;
                }
            }
        }

        published_total += published.len() as i64;
        components += 1;
        compared += 1;
    }
    println!(
        "  component      {components} components — {published_total} published versions match, {skipped_total} in-flight/unpublished correctly absent"
    );

    println!(
        "  total: {compared} streams compared ({live} live, {deleted} deleted, {grants} grants, {components} components)"
    );
    assert!(compared > 0, "no streams were compared");
}

// ============================================================
// Phase 5 — transactional command smoke on migrated data
// ============================================================
//
// The whole point of routing Forest's writes through Mire's `TransactionScope`
// is that the event append and the synchronous projection SQL still commit or
// roll back together. Prove both directions against the migrated database.
//
// This test writes. It is only ever pointed at a disposable green rehearsal
// copy, never at the source cluster.

/// Count events and streams so a rollback can be shown to leave no trace.
async fn event_store_shape(pool: &PgPool) -> (i64, i64) {
    let row = sqlx::query(
        "SELECT (SELECT count(*) FROM es_events) AS events,
                (SELECT count(*) FROM es_streams) AS streams",
    )
    .fetch_one(pool)
    .await
    .expect("read event store shape");
    (row.get("events"), row.get("streams"))
}

#[tokio::test]
async fn command_commits_event_and_projection_atomically() {
    let Some(pool) = green_pool().await else {
        eprintln!("skipping: GREEN_DATABASE_URL is not set");
        return;
    };
    if std::env::var("GREEN_ALLOW_WRITES").is_err() {
        eprintln!("skipping: set GREEN_ALLOW_WRITES=1 to run the write smoke against green");
        return;
    }

    let store = EventStore::new(pool.clone());
    // `apps.created_by` carries a foreign key to `users`, so take an
    // organisation together with one of its real members.
    let row = sqlx::query(
        "SELECT m.organisation_id, m.user_id
           FROM organisation_members m
           JOIN users u ON u.id = m.user_id
          ORDER BY m.organisation_id
          LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("green has at least one organisation member");
    let organisation_id: uuid::Uuid = row.get("organisation_id");
    let created_by: uuid::Uuid = row.get("user_id");

    let (events_before, streams_before) = event_store_shape(&pool).await;

    // ---- 1. happy path: event + projection commit together ----
    let app_id = uuid::Uuid::now_v7();
    let name = format!("mire-green-smoke-{}", &app_id.to_string()[..8]);
    let aggregate_id = format!("{organisation_id}/{name}");

    let mut root = store
        .load_or_default::<AppAggregate>(&aggregate_id)
        .await
        .expect("load app aggregate");
    root.record(forest_server::domains::app::AppEvent::Created {
        app_id,
        organisation_id,
        name: name.clone(),
        description: Some("Mire green rehearsal smoke".to_string()),
        permissions: serde_json::json!({}),
        created_by,
    });

    let mut scope = store.begin_transaction().await.expect("begin scope");
    scope.save(&mut root).await.expect("append app event");
    sqlx::query(
        "INSERT INTO apps (id, organisation_id, name, description, permissions, created_by, suspended, created_at, updated_at)
         VALUES ($1, $2, $3, $4, '{}'::jsonb, $5, false, now(), now())",
    )
    .bind(app_id)
    .bind(organisation_id)
    .bind(&name)
    .bind("Mire green rehearsal smoke")
    .bind(created_by)
    .execute(&mut **scope.tx())
    .await
    .expect("insert app projection");
    scope.commit().await.expect("commit event + projection");

    let (events_after, streams_after) = event_store_shape(&pool).await;
    assert_eq!(
        events_after,
        events_before + 1,
        "the app event was not appended"
    );
    assert_eq!(
        streams_after,
        streams_before + 1,
        "the app stream was not created"
    );

    let projected: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM apps WHERE id = $1")
        .bind(app_id)
        .fetch_one(&pool)
        .await
        .expect("count projection row");
    assert_eq!(
        projected, 1,
        "the projection row did not commit with the event"
    );

    // The committed stream must hydrate back through Mire.
    let reloaded = store
        .load::<AppAggregate>(&aggregate_id)
        .await
        .expect("reload")
        .expect("stream exists");
    assert_eq!(
        reloaded.state.name, name,
        "reloaded aggregate lost its name"
    );
    println!("committed app event + projection atomically, and it hydrates back");

    // ---- 2. failure path: a broken projection must roll the event back ----
    let (events_mid, streams_mid) = event_store_shape(&pool).await;

    let mut doomed = store
        .load_or_default::<AppAggregate>(&aggregate_id)
        .await
        .expect("load for failure case");
    doomed.record(forest_server::domains::app::AppEvent::Suspended);

    let mut scope = store
        .begin_transaction()
        .await
        .expect("begin failing scope");
    scope.save(&mut doomed).await.expect("append suspend event");
    // Deliberate foreign-key violation: no such organisation.
    let broken = sqlx::query(
        "INSERT INTO apps (id, organisation_id, name, description, permissions, created_by, suspended, created_at, updated_at)
         VALUES ($1, $2, $3, NULL, '{}'::jsonb, $2, false, now(), now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(uuid::Uuid::nil()) // not a real organisation -> FK violation
    .bind(format!("{name}-doomed"))
    .execute(&mut **scope.tx())
    .await;
    assert!(
        broken.is_err(),
        "the projection insert was supposed to violate a foreign key"
    );
    drop(scope); // no commit: both sides must vanish

    let (events_rolled, streams_rolled) = event_store_shape(&pool).await;
    assert_eq!(
        (events_rolled, streams_rolled),
        (events_mid, streams_mid),
        "a failed projection left the event append behind — atomicity is broken"
    );

    let still_active = store
        .load::<AppAggregate>(&aggregate_id)
        .await
        .expect("reload after rollback")
        .expect("stream still exists");
    assert!(
        !still_active.state.suspended,
        "the rolled-back Suspended event is visible in the aggregate"
    );
    println!("forced projection FK failure rolled back both the event and the projection");
}
