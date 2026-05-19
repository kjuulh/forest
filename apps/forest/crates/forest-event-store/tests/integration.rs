use forest_event_store::{
    Aggregate, EventData, EventStore, ExpectedVersion, IntoStreamCategory, ReadDirection,
    StreamCategory, StreamQuery, Subscription,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::OnceCell;

// ============================================================
// Test domain: Counter aggregate
// ============================================================

#[derive(Debug, Default, Clone, PartialEq)]
struct Counter {
    value: i64,
    ops: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum CounterEvent {
    Incremented { amount: i64 },
    Decremented { amount: i64 },
    Reset,
}

impl EventData for CounterEvent {
    fn event_type(&self) -> &'static str {
        match self {
            CounterEvent::Incremented { .. } => "counter.incremented",
            CounterEvent::Decremented { .. } => "counter.decremented",
            CounterEvent::Reset => "counter.reset",
        }
    }
}

impl Aggregate for Counter {
    type Event = CounterEvent;

    fn stream_category() -> StreamCategory {
        "counter".into_stream_category()
    }

    fn apply(&mut self, event: &CounterEvent) {
        self.ops += 1;
        match event {
            CounterEvent::Incremented { amount } => self.value += amount,
            CounterEvent::Decremented { amount } => self.value -= amount,
            CounterEvent::Reset => self.value = 0,
        }
    }
}

// ============================================================
// Second domain: Wallet aggregate (for cross-category tests)
// ============================================================

#[derive(Debug, Default)]
struct Wallet {
    balance: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum WalletEvent {
    Deposited { amount: i64 },
    Withdrawn { amount: i64 },
}

impl EventData for WalletEvent {
    fn event_type(&self) -> &'static str {
        match self {
            WalletEvent::Deposited { .. } => "wallet.deposited",
            WalletEvent::Withdrawn { .. } => "wallet.withdrawn",
        }
    }
}

impl Aggregate for Wallet {
    type Event = WalletEvent;

    fn stream_category() -> StreamCategory {
        "wallet".into_stream_category()
    }

    fn apply(&mut self, event: &WalletEvent) {
        match event {
            WalletEvent::Deposited { amount } => self.balance += amount,
            WalletEvent::Withdrawn { amount } => self.balance -= amount,
        }
    }
}

// ============================================================
// Test infrastructure
// ============================================================

fn uid() -> String {
    uuid::Uuid::now_v7().to_string()
}

static DB_POOL: OnceCell<PgPool> = OnceCell::const_new();

async fn get_pool() -> PgPool {
    DB_POOL
        .get_or_init(|| async {
            let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
                dotenvy::dotenv().ok();
                std::env::var("DATABASE_URL").expect("DATABASE_URL must be set")
            });
            PgPool::connect(&database_url)
                .await
                .expect("connect to database")
        })
        .await
        .clone()
}

async fn setup() -> EventStore {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("forest_event_store=debug,info")
        .with_test_writer()
        .try_init();

    let pool = get_pool().await;
    let store = EventStore::new(pool);
    store.migrate().await.expect("run migrations");
    store
}

/// Get the current global max position (to isolate from other tests).
async fn current_max_pos(store: &EventStore) -> i64 {
    store
        .read_all(0, i64::MAX)
        .await
        .unwrap()
        .last()
        .map(|e| e.global_position)
        .unwrap_or(0)
}

// ============================================================
// Aggregate lifecycle
// ============================================================

#[tokio::test]
async fn test_load_nonexistent_returns_none() {
    let store = setup().await;
    let loaded = store.load::<Counter>(&uid()).await.unwrap();
    assert!(loaded.is_none());
}

#[tokio::test]
async fn test_load_or_default_on_nonexistent() {
    let store = setup().await;
    let id = uid();
    let root = store.load_or_default::<Counter>(&id).await.unwrap();
    assert_eq!(root.state, Counter::default());
    assert_eq!(root.version, 0);
    assert_eq!(root.stream_id, format!("counter-{id}"));
}

#[tokio::test]
async fn test_create_save_reload() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 5 });
    root.record(CounterEvent::Incremented { amount: 3 });
    assert_eq!(root.state.value, 8);

    store.save(&mut root).await.unwrap();
    assert_eq!(root.version, 2);

    let loaded = store.load::<Counter>(&id).await.unwrap().unwrap();
    assert_eq!(loaded.state.value, 8);
    assert_eq!(loaded.state.ops, 2);
    assert_eq!(loaded.version, 2);
}

#[tokio::test]
async fn test_multiple_save_rounds() {
    let store = setup().await;
    let id = uid();

    // Round 1
    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 10 });
    store.save(&mut root).await.unwrap();
    assert_eq!(root.version, 1);

    // Round 2: reload and append more
    let mut root = store.load::<Counter>(&id).await.unwrap().unwrap();
    assert_eq!(root.state.value, 10);
    root.record(CounterEvent::Decremented { amount: 3 });
    root.record(CounterEvent::Incremented { amount: 1 });
    store.save(&mut root).await.unwrap();
    assert_eq!(root.version, 3);

    // Round 3
    let mut root = store.load::<Counter>(&id).await.unwrap().unwrap();
    assert_eq!(root.state.value, 8);
    root.record(CounterEvent::Reset);
    store.save(&mut root).await.unwrap();
    assert_eq!(root.version, 4);

    let final_root = store.load::<Counter>(&id).await.unwrap().unwrap();
    assert_eq!(final_root.state.value, 0);
    assert_eq!(final_root.state.ops, 4);
}

#[tokio::test]
async fn test_save_with_no_pending_is_noop() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 1 });
    store.save(&mut root).await.unwrap();

    // Save again with nothing pending
    let mut root = store.load::<Counter>(&id).await.unwrap().unwrap();
    store.save(&mut root).await.unwrap(); // no-op
    assert_eq!(root.version, 1);
}

// ============================================================
// Optimistic concurrency
// ============================================================

#[tokio::test]
async fn test_optimistic_concurrency_conflict() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 1 });
    store.save(&mut root).await.unwrap();

    let mut root_a = store.load::<Counter>(&id).await.unwrap().unwrap();
    let mut root_b = store.load::<Counter>(&id).await.unwrap().unwrap();

    root_a.record(CounterEvent::Incremented { amount: 10 });
    store.save(&mut root_a).await.unwrap();

    root_b.record(CounterEvent::Decremented { amount: 5 });
    let err = store.save(&mut root_b).await.unwrap_err();
    assert!(err.to_string().contains("concurrency conflict"));
}

#[tokio::test]
async fn test_no_stream_prevents_double_create() {
    let store = setup().await;
    let stream_id = format!("counter-{}", uid());

    store
        .append(
            &stream_id,
            "counter",
            ExpectedVersion::NoStream,
            &[CounterEvent::Incremented { amount: 1 }],
        )
        .await
        .unwrap();

    let err = store
        .append(
            &stream_id,
            "counter",
            ExpectedVersion::NoStream,
            &[CounterEvent::Incremented { amount: 1 }],
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("expected no stream"));
}

#[tokio::test]
async fn test_exact_version_on_nonexistent_fails() {
    let store = setup().await;
    let stream_id = format!("counter-{}", uid());

    let err = store
        .append(
            &stream_id,
            "counter",
            ExpectedVersion::Exact(1),
            &[CounterEvent::Incremented { amount: 1 }],
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("concurrency conflict"));
}

#[tokio::test]
async fn test_exact_version_wrong_version_fails() {
    let store = setup().await;
    let stream_id = format!("counter-{}", uid());

    store
        .append(
            &stream_id,
            "counter",
            ExpectedVersion::NoStream,
            &[CounterEvent::Incremented { amount: 1 }],
        )
        .await
        .unwrap();

    let err = store
        .append(
            &stream_id,
            "counter",
            ExpectedVersion::Exact(5),
            &[CounterEvent::Incremented { amount: 1 }],
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("concurrency conflict"));
}

#[tokio::test]
async fn test_exact_version_correct_succeeds() {
    let store = setup().await;
    let stream_id = format!("counter-{}", uid());

    let v = store
        .append(
            &stream_id,
            "counter",
            ExpectedVersion::NoStream,
            &[CounterEvent::Incremented { amount: 1 }],
        )
        .await
        .unwrap();
    assert_eq!(v, 1);

    let v = store
        .append(
            &stream_id,
            "counter",
            ExpectedVersion::Exact(1),
            &[CounterEvent::Incremented { amount: 2 }],
        )
        .await
        .unwrap();
    assert_eq!(v, 2);
}

#[tokio::test]
async fn test_any_version_always_succeeds() {
    let store = setup().await;
    let stream_id = format!("counter-{}", uid());

    for i in 0..5 {
        store
            .append(
                &stream_id,
                "counter",
                ExpectedVersion::Any,
                &[CounterEvent::Incremented { amount: i }],
            )
            .await
            .unwrap();
    }

    let events = store
        .read_stream(&stream_id, &StreamQuery::default())
        .await
        .unwrap();
    assert_eq!(events.len(), 5);
}

#[tokio::test]
async fn test_append_empty_slice_errors() {
    let store = setup().await;
    let stream_id = format!("counter-{}", uid());

    let err = store
        .append::<CounterEvent>(&stream_id, "counter", ExpectedVersion::Any, &[])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("cannot append zero events"));
}

// ============================================================
// Read stream
// ============================================================

#[tokio::test]
async fn test_read_stream_forward_all() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 1 });
    root.record(CounterEvent::Incremented { amount: 2 });
    root.record(CounterEvent::Decremented { amount: 1 });
    root.record(CounterEvent::Reset);
    store.save(&mut root).await.unwrap();

    let events = store
        .read_stream(&root.stream_id, &StreamQuery::default())
        .await
        .unwrap();

    assert_eq!(events.len(), 4);
    assert_eq!(events[0].event_type, "counter.incremented");
    assert_eq!(events[1].event_type, "counter.incremented");
    assert_eq!(events[2].event_type, "counter.decremented");
    assert_eq!(events[3].event_type, "counter.reset");
    assert_eq!(events[0].stream_version, 1);
    assert_eq!(events[3].stream_version, 4);
}

#[tokio::test]
async fn test_read_stream_backward() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 1 });
    root.record(CounterEvent::Incremented { amount: 2 });
    root.record(CounterEvent::Incremented { amount: 3 });
    store.save(&mut root).await.unwrap();

    let events = store
        .read_stream(
            &root.stream_id,
            &StreamQuery {
                direction: ReadDirection::Backward,
                from_version: i64::MAX,
                limit: 1000,
            },
        )
        .await
        .unwrap();

    assert_eq!(events.len(), 3);
    // Backward: version 3, 2, 1
    assert_eq!(events[0].stream_version, 3);
    assert_eq!(events[1].stream_version, 2);
    assert_eq!(events[2].stream_version, 1);
}

#[tokio::test]
async fn test_read_stream_with_from_version() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    for i in 1..=5 {
        root.record(CounterEvent::Incremented { amount: i });
    }
    store.save(&mut root).await.unwrap();

    // Read from version 3 onward
    let events = store
        .read_stream(
            &root.stream_id,
            &StreamQuery {
                direction: ReadDirection::Forward,
                from_version: 3,
                limit: 1000,
            },
        )
        .await
        .unwrap();

    assert_eq!(events.len(), 3);
    assert_eq!(events[0].stream_version, 3);
    assert_eq!(events[2].stream_version, 5);
}

#[tokio::test]
async fn test_read_stream_with_limit() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    for i in 1..=10 {
        root.record(CounterEvent::Incremented { amount: i });
    }
    store.save(&mut root).await.unwrap();

    let events = store
        .read_stream(
            &root.stream_id,
            &StreamQuery {
                direction: ReadDirection::Forward,
                from_version: 0,
                limit: 3,
            },
        )
        .await
        .unwrap();

    assert_eq!(events.len(), 3);
    assert_eq!(events[0].stream_version, 1);
    assert_eq!(events[2].stream_version, 3);
}

#[tokio::test]
async fn test_read_stream_backward_with_from_version() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    for i in 1..=5 {
        root.record(CounterEvent::Incremented { amount: i });
    }
    store.save(&mut root).await.unwrap();

    // Read backward from version 3
    let events = store
        .read_stream(
            &root.stream_id,
            &StreamQuery {
                direction: ReadDirection::Backward,
                from_version: 3,
                limit: 1000,
            },
        )
        .await
        .unwrap();

    assert_eq!(events.len(), 3); // versions 3, 2, 1
    assert_eq!(events[0].stream_version, 3);
    assert_eq!(events[2].stream_version, 1);
}

#[tokio::test]
async fn test_read_nonexistent_stream_returns_empty() {
    let store = setup().await;

    let events = store
        .read_stream(&format!("counter-{}", uid()), &StreamQuery::default())
        .await
        .unwrap();
    assert!(events.is_empty());
}

// ============================================================
// Read all (global log)
// ============================================================

#[tokio::test]
async fn test_read_all_global_ordering() {
    let store = setup().await;
    let start = current_max_pos(&store).await;

    let id_a = uid();
    let id_b = uid();

    let mut root_a = store.load_or_default::<Counter>(&id_a).await.unwrap();
    root_a.record(CounterEvent::Incremented { amount: 10 });
    store.save(&mut root_a).await.unwrap();

    let mut root_b = store.load_or_default::<Counter>(&id_b).await.unwrap();
    root_b.record(CounterEvent::Incremented { amount: 20 });
    store.save(&mut root_b).await.unwrap();

    let events = store.read_all(start, 100).await.unwrap();
    assert!(events.len() >= 2);

    for w in events.windows(2) {
        assert!(w[1].global_position > w[0].global_position);
    }
}

#[tokio::test]
async fn test_read_all_with_limit() {
    let store = setup().await;
    let start = current_max_pos(&store).await;

    let id = uid();
    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    for i in 1..=5 {
        root.record(CounterEvent::Incremented { amount: i });
    }
    store.save(&mut root).await.unwrap();

    let events = store.read_all(start, 2).await.unwrap();
    assert_eq!(events.len(), 2);
}

#[tokio::test]
async fn test_read_all_pagination() {
    let store = setup().await;
    let start = current_max_pos(&store).await;

    let id = uid();
    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    for i in 1..=7 {
        root.record(CounterEvent::Incremented { amount: i });
    }
    store.save(&mut root).await.unwrap();

    // Page through 3 at a time
    let mut all = Vec::new();
    let mut pos = start;
    loop {
        let batch = store.read_all(pos, 3).await.unwrap();
        if batch.is_empty() {
            break;
        }
        pos = batch.last().unwrap().global_position;
        all.extend(batch);
    }

    assert!(all.len() >= 7);
    // Verify no duplicates
    let positions: Vec<_> = all.iter().map(|e| e.global_position).collect();
    for w in positions.windows(2) {
        assert!(w[1] > w[0], "positions must be strictly increasing");
    }
}

// ============================================================
// Read category
// ============================================================

#[tokio::test]
async fn test_read_category_filters_correctly() {
    let store = setup().await;
    let start = current_max_pos(&store).await;

    // Create counter events
    let id = uid();
    let mut counter = store.load_or_default::<Counter>(&id).await.unwrap();
    counter.record(CounterEvent::Incremented { amount: 1 });
    store.save(&mut counter).await.unwrap();

    // Create wallet events
    let id2 = uid();
    let mut wallet = store.load_or_default::<Wallet>(&id2).await.unwrap();
    wallet.record(WalletEvent::Deposited { amount: 100 });
    store.save(&mut wallet).await.unwrap();

    // Counter category only
    let counter_events = store.read_category("counter", start, 100).await.unwrap();
    assert!(!counter_events.is_empty());
    assert!(counter_events
        .iter()
        .all(|e| e.stream_id.starts_with("counter-")));

    // Wallet category only
    let wallet_events = store.read_category("wallet", start, 100).await.unwrap();
    assert!(!wallet_events.is_empty());
    assert!(wallet_events
        .iter()
        .all(|e| e.stream_id.starts_with("wallet-")));
}

#[tokio::test]
async fn test_read_category_nonexistent_returns_empty() {
    let store = setup().await;
    let start = current_max_pos(&store).await;

    let events = store
        .read_category("nonexistent_category", start, 100)
        .await
        .unwrap();
    assert!(events.is_empty());
}

// ============================================================
// Event data integrity
// ============================================================

#[tokio::test]
async fn test_event_data_roundtrip_through_db() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: i64::MAX });
    root.record(CounterEvent::Reset);
    root.record(CounterEvent::Decremented { amount: i64::MAX });
    root.record(CounterEvent::Reset);
    store.save(&mut root).await.unwrap();

    let events = store
        .read_stream(&root.stream_id, &StreamQuery::default())
        .await
        .unwrap();

    let e0: CounterEvent = serde_json::from_value(events[0].data.clone()).unwrap();
    let e2: CounterEvent = serde_json::from_value(events[2].data.clone()).unwrap();
    let e3: CounterEvent = serde_json::from_value(events[3].data.clone()).unwrap();

    assert!(matches!(e0, CounterEvent::Incremented { amount } if amount == i64::MAX));
    assert!(matches!(e2, CounterEvent::Decremented { amount } if amount == i64::MAX));
    assert!(matches!(e3, CounterEvent::Reset));
}

#[tokio::test]
async fn test_event_types_stored_correctly() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 1 });
    root.record(CounterEvent::Decremented { amount: 1 });
    root.record(CounterEvent::Reset);
    store.save(&mut root).await.unwrap();

    let events = store
        .read_stream(&root.stream_id, &StreamQuery::default())
        .await
        .unwrap();

    assert_eq!(events[0].event_type, "counter.incremented");
    assert_eq!(events[1].event_type, "counter.decremented");
    assert_eq!(events[2].event_type, "counter.reset");
}

#[tokio::test]
async fn test_stream_version_monotonic() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    for i in 1..=20 {
        root.record(CounterEvent::Incremented { amount: i });
    }
    store.save(&mut root).await.unwrap();

    let events = store
        .read_stream(&root.stream_id, &StreamQuery::default())
        .await
        .unwrap();

    assert_eq!(events.len(), 20);
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e.stream_version, (i + 1) as i64);
    }
}

#[tokio::test]
async fn test_events_have_timestamps() {
    let store = setup().await;
    let id = uid();
    let before = chrono::Utc::now();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 1 });
    store.save(&mut root).await.unwrap();

    let after = chrono::Utc::now();

    let events = store
        .read_stream(&root.stream_id, &StreamQuery::default())
        .await
        .unwrap();

    assert_eq!(events.len(), 1);
    assert!(events[0].created_at >= before);
    assert!(events[0].created_at <= after);
}

#[tokio::test]
async fn test_metadata_defaults_to_empty_object() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 1 });
    store.save(&mut root).await.unwrap();

    let events = store
        .read_stream(&root.stream_id, &StreamQuery::default())
        .await
        .unwrap();

    assert!(events[0].metadata.is_object());
    assert_eq!(events[0].metadata.as_object().unwrap().len(), 0);
}

// ============================================================
// Multi-aggregate / cross-category
// ============================================================

#[tokio::test]
async fn test_two_aggregates_same_category_independent() {
    let store = setup().await;
    let id_a = uid();
    let id_b = uid();

    let mut a = store.load_or_default::<Counter>(&id_a).await.unwrap();
    a.record(CounterEvent::Incremented { amount: 100 });
    store.save(&mut a).await.unwrap();

    let mut b = store.load_or_default::<Counter>(&id_b).await.unwrap();
    b.record(CounterEvent::Incremented { amount: 200 });
    store.save(&mut b).await.unwrap();

    let a2 = store.load::<Counter>(&id_a).await.unwrap().unwrap();
    let b2 = store.load::<Counter>(&id_b).await.unwrap().unwrap();

    assert_eq!(a2.state.value, 100);
    assert_eq!(b2.state.value, 200);
}

#[tokio::test]
async fn test_different_aggregate_types() {
    let store = setup().await;
    let id = uid();

    let mut counter = store.load_or_default::<Counter>(&id).await.unwrap();
    counter.record(CounterEvent::Incremented { amount: 5 });
    store.save(&mut counter).await.unwrap();

    let mut wallet = store.load_or_default::<Wallet>(&id).await.unwrap();
    wallet.record(WalletEvent::Deposited { amount: 1000 });
    store.save(&mut wallet).await.unwrap();

    // They use different stream categories so different stream IDs
    let c = store.load::<Counter>(&id).await.unwrap().unwrap();
    let w = store.load::<Wallet>(&id).await.unwrap().unwrap();
    assert_eq!(c.state.value, 5);
    assert_eq!(w.state.balance, 1000);
}

// ============================================================
// Large batches
// ============================================================

#[tokio::test]
async fn test_large_batch_append() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    for i in 1..=500 {
        root.record(CounterEvent::Incremented { amount: i });
    }
    store.save(&mut root).await.unwrap();
    assert_eq!(root.version, 500);

    // Sum 1..=500 = 125250
    let loaded = store.load::<Counter>(&id).await.unwrap().unwrap();
    assert_eq!(loaded.state.value, 125250);
    assert_eq!(loaded.state.ops, 500);
}

// ============================================================
// Subscription
// ============================================================

#[tokio::test]
async fn test_subscription_poll_and_checkpoint() {
    let store = setup().await;
    let sub_id = format!("test-sub-{}", uid());

    let mut sub = Subscription::create(store.clone(), store.pool().clone(), &sub_id, 100)
        .await
        .unwrap();

    // Drain pre-existing events
    while !sub.poll().await.unwrap().is_empty() {}
    sub.checkpoint().await.unwrap();
    let pos_before = sub.position();

    // Add events
    let id = uid();
    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 1 });
    root.record(CounterEvent::Incremented { amount: 2 });
    store.save(&mut root).await.unwrap();

    let events = sub.poll().await.unwrap();
    assert!(events.len() >= 2);
    assert!(sub.position() > pos_before);

    sub.checkpoint().await.unwrap();
    let checkpointed = sub.position();

    // Drain anything from concurrent tests
    while !sub.poll().await.unwrap().is_empty() {}
    sub.checkpoint().await.unwrap();

    // Resume from checkpoint
    let sub2 = Subscription::create(store.clone(), store.pool().clone(), &sub_id, 100)
        .await
        .unwrap();
    assert!(sub2.position() >= checkpointed);
}

#[tokio::test]
async fn test_subscription_poll_category() {
    let store = setup().await;
    let sub_id = format!("test-cat-sub-{}", uid());

    let mut sub = Subscription::create(store.clone(), store.pool().clone(), &sub_id, 100)
        .await
        .unwrap();

    // Drain existing
    while !sub.poll().await.unwrap().is_empty() {}
    sub.checkpoint().await.unwrap();

    // Create counter + wallet events
    let id = uid();
    let mut counter = store.load_or_default::<Counter>(&id).await.unwrap();
    counter.record(CounterEvent::Incremented { amount: 1 });
    store.save(&mut counter).await.unwrap();

    let id2 = uid();
    let mut wallet = store.load_or_default::<Wallet>(&id2).await.unwrap();
    wallet.record(WalletEvent::Deposited { amount: 50 });
    store.save(&mut wallet).await.unwrap();

    // Category subscription: only counter
    let events = sub.poll_category("counter").await.unwrap();
    assert!(events.iter().all(|e| e.stream_id.starts_with("counter-")));
    // Should have at least our 1 counter event
    assert!(!events.is_empty());
}

#[tokio::test]
async fn test_subscription_small_batch_paginated() {
    let store = setup().await;
    let sub_id = format!("test-page-sub-{}", uid());

    let mut sub = Subscription::create(store.clone(), store.pool().clone(), &sub_id, 2)
        .await
        .unwrap();

    // Drain
    while !sub.poll().await.unwrap().is_empty() {}
    sub.checkpoint().await.unwrap();

    // Add 5 events
    let id = uid();
    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    for i in 1..=5 {
        root.record(CounterEvent::Incremented { amount: i });
    }
    store.save(&mut root).await.unwrap();

    // Poll in batches of 2
    let mut total = 0;
    loop {
        let batch = sub.poll().await.unwrap();
        if batch.is_empty() {
            break;
        }
        assert!(batch.len() <= 2);
        total += batch.len();
    }
    assert!(total >= 5);
}

// ============================================================
// Concurrent appends (tokio tasks)
// ============================================================

#[tokio::test]
async fn test_concurrent_appends_with_any_version() {
    let store = setup().await;
    let stream_id = format!("counter-{}", uid());

    let mut handles = Vec::new();
    for i in 0..10 {
        let s = store.clone();
        let sid = stream_id.clone();
        handles.push(tokio::spawn(async move {
            s.append(
                &sid,
                "counter",
                ExpectedVersion::Any,
                &[CounterEvent::Incremented { amount: i }],
            )
            .await
        }));
    }

    for h in handles {
        h.await.unwrap().unwrap();
    }

    let events = store
        .read_stream(&stream_id, &StreamQuery::default())
        .await
        .unwrap();
    assert_eq!(events.len(), 10);

    // Versions must be 1..=10, sequential
    let versions: Vec<i64> = events.iter().map(|e| e.stream_version).collect();
    for (i, v) in versions.iter().enumerate() {
        assert_eq!(*v, (i + 1) as i64);
    }
}

#[tokio::test]
async fn test_concurrent_exact_version_one_wins() {
    let store = setup().await;
    let id = uid();

    // Create initial
    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 1 });
    store.save(&mut root).await.unwrap();

    let stream_id = root.stream_id.clone();

    // Race 5 tasks all trying Exact(1)
    let mut handles = Vec::new();
    for i in 0..5 {
        let s = store.clone();
        let sid = stream_id.clone();
        handles.push(tokio::spawn(async move {
            s.append(
                &sid,
                "counter",
                ExpectedVersion::Exact(1),
                &[CounterEvent::Incremented { amount: i }],
            )
            .await
        }));
    }

    let mut successes = 0;
    let mut failures = 0;
    for h in handles {
        match h.await.unwrap() {
            Ok(_) => successes += 1,
            Err(_) => failures += 1,
        }
    }

    // Exactly one should win
    assert_eq!(successes, 1);
    assert_eq!(failures, 4);
}

// ============================================================
// Edge cases
// ============================================================

#[tokio::test]
async fn test_single_event_stream() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 42 });
    store.save(&mut root).await.unwrap();

    let loaded = store.load::<Counter>(&id).await.unwrap().unwrap();
    assert_eq!(loaded.state.value, 42);
    assert_eq!(loaded.version, 1);
}

#[tokio::test]
async fn test_many_small_saves() {
    let store = setup().await;
    let id = uid();

    for i in 1..=50 {
        let mut root = if i == 1 {
            store.load_or_default::<Counter>(&id).await.unwrap()
        } else {
            store.load::<Counter>(&id).await.unwrap().unwrap()
        };
        root.record(CounterEvent::Incremented { amount: 1 });
        store.save(&mut root).await.unwrap();
    }

    let loaded = store.load::<Counter>(&id).await.unwrap().unwrap();
    assert_eq!(loaded.state.value, 50);
    assert_eq!(loaded.version, 50);
}

#[tokio::test]
async fn test_version_returned_from_append_matches() {
    let store = setup().await;
    let stream_id = format!("counter-{}", uid());

    let v1 = store
        .append(
            &stream_id,
            "counter",
            ExpectedVersion::NoStream,
            &[
                CounterEvent::Incremented { amount: 1 },
                CounterEvent::Incremented { amount: 2 },
                CounterEvent::Incremented { amount: 3 },
            ],
        )
        .await
        .unwrap();
    assert_eq!(v1, 3);

    let v2 = store
        .append(
            &stream_id,
            "counter",
            ExpectedVersion::Exact(3),
            &[CounterEvent::Reset],
        )
        .await
        .unwrap();
    assert_eq!(v2, 4);
}

#[tokio::test]
async fn test_aggregate_root_stream_id_format() {
    let store = setup().await;
    let id = uid();

    let root = store.load_or_default::<Counter>(&id).await.unwrap();
    assert_eq!(root.stream_id, format!("counter-{}", id));

    let root = store.load_or_default::<Wallet>(&id).await.unwrap();
    assert_eq!(root.stream_id, format!("wallet-{}", id));
}

#[tokio::test]
async fn test_global_position_always_increases() {
    let store = setup().await;
    let start = current_max_pos(&store).await;

    // Create events across multiple streams
    for _ in 0..5 {
        let id = uid();
        let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
        root.record(CounterEvent::Incremented { amount: 1 });
        store.save(&mut root).await.unwrap();
    }

    let events = store.read_all(start, 100).await.unwrap();
    assert!(events.len() >= 5);

    for w in events.windows(2) {
        assert!(
            w[1].global_position > w[0].global_position,
            "global_position must be strictly increasing: {} should be > {}",
            w[1].global_position,
            w[0].global_position
        );
    }
}

#[tokio::test]
async fn test_stream_id_in_events_matches() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 1 });
    store.save(&mut root).await.unwrap();

    let events = store
        .read_stream(&root.stream_id, &StreamQuery::default())
        .await
        .unwrap();

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].stream_id, root.stream_id);
}

// ============================================================
// Idempotent migration
// ============================================================

#[tokio::test]
async fn test_migrate_is_idempotent() {
    let store = setup().await;
    // Run migrate again — should not error
    store.migrate().await.unwrap();
    store.migrate().await.unwrap();

    // Store still works
    let id = uid();
    let mut root = store.load_or_default::<Counter>(&id).await.unwrap();
    root.record(CounterEvent::Incremented { amount: 1 });
    store.save(&mut root).await.unwrap();
}
