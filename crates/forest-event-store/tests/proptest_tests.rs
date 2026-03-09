use forest_event_store::{
    Aggregate, AggregateRoot, EventData, EventStore, IntoStreamCategory, ReadDirection,
    RecordedEvent, StreamCategory, StreamQuery,
};
use proptest::prelude::*;
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::OnceCell;

// ============================================================
// Test domain
// ============================================================

#[derive(Debug, Default, Clone, PartialEq)]
struct Ledger {
    balance: i64,
    tx_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
enum LedgerEvent {
    Credited { amount: i64 },
    Debited { amount: i64 },
    Zeroed,
}

impl EventData for LedgerEvent {
    fn event_type(&self) -> &'static str {
        match self {
            LedgerEvent::Credited { .. } => "ledger.credited",
            LedgerEvent::Debited { .. } => "ledger.debited",
            LedgerEvent::Zeroed => "ledger.zeroed",
        }
    }
}

impl Aggregate for Ledger {
    type Event = LedgerEvent;

    fn stream_category() -> StreamCategory {
        "propledger".into_stream_category()
    }

    fn apply(&mut self, event: &LedgerEvent) {
        self.tx_count += 1;
        match event {
            LedgerEvent::Credited { amount } => self.balance = self.balance.saturating_add(*amount),
            LedgerEvent::Debited { amount } => self.balance = self.balance.saturating_sub(*amount),
            LedgerEvent::Zeroed => self.balance = 0,
        }
    }
}

/// Pure fold — apply events to a default aggregate.
fn fold_events(events: &[LedgerEvent]) -> Ledger {
    let mut state = Ledger::default();
    for e in events {
        state.apply(e);
    }
    state
}

// ============================================================
// Proptest strategies
// ============================================================

fn ledger_event_strategy() -> impl Strategy<Value = LedgerEvent> {
    prop_oneof![
        (1..=10000i64).prop_map(|amount| LedgerEvent::Credited { amount }),
        (1..=10000i64).prop_map(|amount| LedgerEvent::Debited { amount }),
        Just(LedgerEvent::Zeroed),
    ]
}

fn event_sequence_strategy(max_len: usize) -> impl Strategy<Value = Vec<LedgerEvent>> {
    prop::collection::vec(ledger_event_strategy(), 1..=max_len)
}

// ============================================================
// Infrastructure
// ============================================================

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
    let pool = get_pool().await;
    let store = EventStore::new(pool);
    store.migrate().await.expect("run migrations");
    store
}

fn uid() -> String {
    uuid::Uuid::now_v7().to_string()
}

fn random_event(rng: &mut impl Rng) -> LedgerEvent {
    match rng.random_range(0u8..3) {
        0 => LedgerEvent::Credited {
            amount: rng.random_range(1..=10000i64),
        },
        1 => LedgerEvent::Debited {
            amount: rng.random_range(1..=10000i64),
        },
        _ => LedgerEvent::Zeroed,
    }
}

// ============================================================
// Pure property tests (no DB)
// ============================================================

proptest! {
    /// Any sequence of events applied twice gives the same result.
    #[test]
    fn fold_is_deterministic(events in event_sequence_strategy(50)) {
        let state1 = fold_events(&events);
        let state2 = fold_events(&events);
        prop_assert_eq!(state1, state2);
    }

    /// EventData serde roundtrip preserves the event exactly.
    #[test]
    fn event_data_serde_roundtrip(event in ledger_event_strategy()) {
        let json = serde_json::to_value(&event).unwrap();
        let back: LedgerEvent = serde_json::from_value(json).unwrap();
        prop_assert_eq!(event, back);
    }

    /// Splitting a sequence and folding in two parts gives the same result
    /// as folding the whole thing (fold associativity).
    #[test]
    fn fold_split_is_equivalent(
        events in event_sequence_strategy(30),
        split_at in 0..30usize,
    ) {
        let split_at = split_at.min(events.len());
        let (first, second) = events.split_at(split_at);

        let whole = fold_events(&events);

        let mut partial = fold_events(first);
        for e in second {
            partial.apply(e);
        }

        prop_assert_eq!(whole.balance, partial.balance);
    }

    /// AggregateRoot.record() keeps state consistent with direct fold.
    #[test]
    fn aggregate_root_matches_direct_fold(events in event_sequence_strategy(30)) {
        let mut root = AggregateRoot::<Ledger>::new("propledger-test".into());
        for e in &events {
            root.record(e.clone());
        }
        let expected = fold_events(&events);
        prop_assert_eq!(root.state.balance, expected.balance);
        prop_assert_eq!(root.state.tx_count, expected.tx_count);
        prop_assert_eq!(root.pending_count(), events.len());
    }

    /// Hydrating from RecordedEvents gives the same state as direct fold.
    #[test]
    fn hydrate_matches_fold(events in event_sequence_strategy(30)) {
        let recorded: Vec<RecordedEvent> = events
            .iter()
            .enumerate()
            .map(|(i, e)| RecordedEvent {
                global_position: (i + 1) as i64,
                stream_id: "propledger-test".into(),
                stream_version: (i + 1) as i64,
                event_type: e.event_type().into(),
                data: serde_json::to_value(e).unwrap(),
                metadata: Value::Object(Default::default()),
                created_at: chrono::Utc::now(),
            })
            .collect();

        let root = AggregateRoot::<Ledger>::hydrate(
            "propledger-test".into(),
            &recorded,
            events.len() as i64,
        );

        let expected = fold_events(&events);
        prop_assert_eq!(root.state.balance, expected.balance);
        prop_assert_eq!(root.state.tx_count, expected.tx_count);
    }

    /// take_pending returns exactly the events that were recorded.
    #[test]
    fn take_pending_returns_all_recorded(events in event_sequence_strategy(20)) {
        let mut root = AggregateRoot::<Ledger>::new("propledger-test".into());
        for e in &events {
            root.record(e.clone());
        }
        let taken = root.take_pending();
        prop_assert_eq!(taken.len(), events.len());
        prop_assert!(!root.has_pending());
        let expected = fold_events(&events);
        prop_assert_eq!(root.state.balance, expected.balance);
    }

    /// Credit then debit of same amount nets to zero.
    #[test]
    fn credit_debit_symmetry(amount in 0i64..=10000) {
        let events = vec![
            LedgerEvent::Credited { amount },
            LedgerEvent::Debited { amount },
        ];
        let state = fold_events(&events);
        prop_assert_eq!(state.balance, 0);
    }

    /// Zeroed always resets regardless of history.
    #[test]
    fn zeroed_always_resets(events in event_sequence_strategy(20)) {
        let mut all = events;
        all.push(LedgerEvent::Zeroed);
        let state = fold_events(&all);
        prop_assert_eq!(state.balance, 0);
    }

    /// Event type string never changes across serialization.
    #[test]
    fn event_type_stable(event in ledger_event_strategy()) {
        let t1 = event.event_type();
        let json = serde_json::to_value(&event).unwrap();
        let back: LedgerEvent = serde_json::from_value(json).unwrap();
        prop_assert_eq!(t1, back.event_type());
    }
}

// ============================================================
// DB-backed property tests
// ============================================================

/// Property: save then load always gives state matching direct fold.
#[tokio::test]
async fn prop_save_load_consistency() {
    let store = setup().await;
    let mut rng = rand::rng();

    for _ in 0..20 {
        let id = uid();
        let num_events = rng.random_range(1usize..=30);

        let mut events = Vec::new();
        for _ in 0..num_events {
            events.push(random_event(&mut rng));
        }

        let mut root = store.load_or_default::<Ledger>(&id).await.unwrap();
        for e in &events {
            root.record(e.clone());
        }
        store.save(&mut root).await.unwrap();

        let loaded = store.load::<Ledger>(&id).await.unwrap().unwrap();
        let expected = fold_events(&events);

        assert_eq!(loaded.state.balance, expected.balance);
        assert_eq!(loaded.state.tx_count, expected.tx_count);
        assert_eq!(loaded.version, num_events as i64);
    }
}

/// Property: multi-round save/load preserves accumulated state.
#[tokio::test]
async fn prop_multi_round_save_load() {
    let store = setup().await;
    let id = uid();
    let mut rng = rand::rng();

    let mut all_events = Vec::new();

    for round in 0..10 {
        let num_events = rng.random_range(1usize..=5);

        let mut root = if round == 0 {
            store.load_or_default::<Ledger>(&id).await.unwrap()
        } else {
            store.load::<Ledger>(&id).await.unwrap().unwrap()
        };

        for _ in 0..num_events {
            let event = random_event(&mut rng);
            root.record(event.clone());
            all_events.push(event);
        }

        store.save(&mut root).await.unwrap();
    }

    let loaded = store.load::<Ledger>(&id).await.unwrap().unwrap();
    let expected = fold_events(&all_events);

    assert_eq!(loaded.state.balance, expected.balance);
    assert_eq!(loaded.state.tx_count, expected.tx_count);
    assert_eq!(loaded.version, all_events.len() as i64);
}

/// Property: reading all events for a stream and folding matches loaded state.
#[tokio::test]
async fn prop_read_stream_fold_matches_load() {
    let store = setup().await;
    let id = uid();
    let mut rng = rand::rng();

    let mut root = store.load_or_default::<Ledger>(&id).await.unwrap();
    let num_events = rng.random_range(5usize..=25);
    for _ in 0..num_events {
        root.record(random_event(&mut rng));
    }
    store.save(&mut root).await.unwrap();

    let events = store
        .read_stream(&root.stream_id, &StreamQuery::default())
        .await
        .unwrap();

    let mut manual = Ledger::default();
    for recorded in &events {
        let event: LedgerEvent = serde_json::from_value(recorded.data.clone()).unwrap();
        manual.apply(&event);
    }

    let loaded = store.load::<Ledger>(&id).await.unwrap().unwrap();
    assert_eq!(manual.balance, loaded.state.balance);
    assert_eq!(manual.tx_count, loaded.state.tx_count);
}

/// Property: forward read then backward read gives reversed versions.
#[tokio::test]
async fn prop_forward_backward_mirror() {
    let store = setup().await;
    let id = uid();

    let mut root = store.load_or_default::<Ledger>(&id).await.unwrap();
    for i in 1..=10 {
        root.record(LedgerEvent::Credited { amount: i });
    }
    store.save(&mut root).await.unwrap();

    let forward = store
        .read_stream(&root.stream_id, &StreamQuery::default())
        .await
        .unwrap();

    let backward = store
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

    assert_eq!(forward.len(), backward.len());

    let fwd_versions: Vec<i64> = forward.iter().map(|e| e.stream_version).collect();
    let bwd_versions: Vec<i64> = backward.iter().map(|e| e.stream_version).collect();
    let mut reversed = bwd_versions;
    reversed.reverse();
    assert_eq!(fwd_versions, reversed);
}
