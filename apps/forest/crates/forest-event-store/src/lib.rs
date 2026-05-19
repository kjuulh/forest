//! # forest-event-store
//!
//! A PostgreSQL-backed event store for event-sourced aggregates.
//!
//! Core concepts (inspired by EventStore/Kurrent):
//! - **Streams**: Named sequences of events, one per aggregate instance
//! - **Events**: Immutable facts appended to streams with optimistic concurrency
//! - **Aggregates**: State derived by folding events (projections)
//! - **Subscriptions**: Catch-up consumers that track position in the global log

mod event;
mod store;
mod stream;
mod subscription;

pub use event::{Event, EventData, RecordedEvent};
pub use store::EventStore;
pub use stream::{ExpectedVersion, ReadDirection, StreamQuery};
pub use subscription::Subscription;

// Re-export sqlx transaction types for use with `save_with`
pub use sqlx::{PgPool, Postgres, Transaction};

/// A stream category value (e.g. "order", "user").
///
/// Wraps a string that identifies the category prefix for aggregate streams.
/// Stream IDs are formed as `"{category}-{id}"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamCategory(String);

impl StreamCategory {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for StreamCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Trait for types that can be converted into a [`StreamCategory`].
pub trait IntoStreamCategory {
    fn into_stream_category(self) -> StreamCategory;
}

impl IntoStreamCategory for &'static str {
    fn into_stream_category(self) -> StreamCategory {
        StreamCategory(self.to_string())
    }
}

impl IntoStreamCategory for String {
    fn into_stream_category(self) -> StreamCategory {
        StreamCategory(self)
    }
}

impl IntoStreamCategory for StreamCategory {
    fn into_stream_category(self) -> StreamCategory {
        self
    }
}

/// Trait for aggregates whose state is derived from events.
///
/// Implement this on your domain aggregate. The event store will use
/// `apply` to fold events into state when loading an aggregate.
pub trait Aggregate: Default + Send + Sync {
    /// The concrete event enum for this aggregate.
    type Event: EventData;

    /// The stream category prefix (e.g. "order", "user").
    /// Stream IDs are formed as `"{category}-{id}"`.
    fn stream_category() -> StreamCategory;

    /// Apply a single event to mutate aggregate state.
    fn apply(&mut self, event: &Self::Event);
}

/// Loaded aggregate with its current version, ready for command handling.
pub struct AggregateRoot<A: Aggregate> {
    pub state: A,
    pub version: i64,
    pub stream_id: String,
    pending_events: Vec<A::Event>,
}

impl<A: Aggregate> AggregateRoot<A> {
    pub fn new(stream_id: String) -> Self {
        Self {
            state: A::default(),
            version: 0,
            stream_id,
            pending_events: Vec::new(),
        }
    }

    pub fn hydrate(stream_id: String, events: &[RecordedEvent], version: i64) -> Self {
        let mut state = A::default();
        for recorded in events {
            if let Ok(event) = serde_json::from_value::<A::Event>(recorded.data.clone()) {
                state.apply(&event);
            }
        }
        Self {
            state,
            version,
            stream_id,
            pending_events: Vec::new(),
        }
    }

    /// Record a new event (not yet persisted).
    /// The event is applied immediately to keep state consistent.
    pub fn record(&mut self, event: A::Event) {
        self.state.apply(&event);
        self.pending_events.push(event);
    }

    /// Take pending events for persistence, clearing the buffer.
    pub fn take_pending(&mut self) -> Vec<A::Event> {
        std::mem::take(&mut self.pending_events)
    }

    /// Whether there are unpersisted events.
    pub fn has_pending(&self) -> bool {
        !self.pending_events.is_empty()
    }

    /// Number of pending (unpersisted) events.
    pub fn pending_count(&self) -> usize {
        self.pending_events.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use serde_json::Value;

    // ---- Minimal test aggregate ----

    #[derive(Debug, Default, PartialEq)]
    struct TestAgg {
        total: i64,
        event_count: usize,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "t")]
    enum TestEvent {
        Added { n: i64 },
        Cleared,
    }

    impl EventData for TestEvent {
        fn event_type(&self) -> &'static str {
            match self {
                TestEvent::Added { .. } => "test.added",
                TestEvent::Cleared => "test.cleared",
            }
        }
    }

    impl Aggregate for TestAgg {
        type Event = TestEvent;
        fn stream_category() -> StreamCategory {
            "testagg".into_stream_category()
        }
        fn apply(&mut self, event: &TestEvent) {
            self.event_count += 1;
            match event {
                TestEvent::Added { n } => self.total += n,
                TestEvent::Cleared => self.total = 0,
            }
        }
    }

    // ---- AggregateRoot unit tests ----

    #[test]
    fn new_root_has_default_state() {
        let root = AggregateRoot::<TestAgg>::new("testagg-1".into());
        assert_eq!(root.state.total, 0);
        assert_eq!(root.state.event_count, 0);
        assert_eq!(root.version, 0);
        assert!(!root.has_pending());
        assert_eq!(root.pending_count(), 0);
    }

    #[test]
    fn record_applies_event_immediately() {
        let mut root = AggregateRoot::<TestAgg>::new("testagg-1".into());
        root.record(TestEvent::Added { n: 5 });
        assert_eq!(root.state.total, 5);
        assert_eq!(root.state.event_count, 1);
        assert!(root.has_pending());
        assert_eq!(root.pending_count(), 1);
    }

    #[test]
    fn record_multiple_events_accumulates() {
        let mut root = AggregateRoot::<TestAgg>::new("testagg-1".into());
        root.record(TestEvent::Added { n: 3 });
        root.record(TestEvent::Added { n: 7 });
        root.record(TestEvent::Cleared);
        root.record(TestEvent::Added { n: 1 });
        assert_eq!(root.state.total, 1); // 3+7=10, clear=0, +1=1
        assert_eq!(root.state.event_count, 4);
        assert_eq!(root.pending_count(), 4);
    }

    #[test]
    fn take_pending_clears_buffer() {
        let mut root = AggregateRoot::<TestAgg>::new("testagg-1".into());
        root.record(TestEvent::Added { n: 1 });
        root.record(TestEvent::Added { n: 2 });

        let taken = root.take_pending();
        assert_eq!(taken.len(), 2);
        assert!(!root.has_pending());
        assert_eq!(root.pending_count(), 0);
        // State is still mutated
        assert_eq!(root.state.total, 3);
    }

    #[test]
    fn take_pending_when_empty() {
        let mut root = AggregateRoot::<TestAgg>::new("testagg-1".into());
        let taken = root.take_pending();
        assert!(taken.is_empty());
    }

    #[test]
    fn hydrate_replays_events() {
        let events = vec![
            RecordedEvent {
                global_position: 1,
                stream_id: "testagg-1".into(),
                stream_version: 1,
                event_type: "test.added".into(),
                data: serde_json::json!({"t": "Added", "n": 10}),
                metadata: Value::Object(Default::default()),
                created_at: chrono::Utc::now(),
            },
            RecordedEvent {
                global_position: 2,
                stream_id: "testagg-1".into(),
                stream_version: 2,
                event_type: "test.added".into(),
                data: serde_json::json!({"t": "Added", "n": 20}),
                metadata: Value::Object(Default::default()),
                created_at: chrono::Utc::now(),
            },
        ];

        let root = AggregateRoot::<TestAgg>::hydrate("testagg-1".into(), &events, 2);
        assert_eq!(root.state.total, 30);
        assert_eq!(root.state.event_count, 2);
        assert_eq!(root.version, 2);
        assert!(!root.has_pending());
    }

    #[test]
    fn hydrate_skips_invalid_events() {
        let events = vec![
            RecordedEvent {
                global_position: 1,
                stream_id: "testagg-1".into(),
                stream_version: 1,
                event_type: "test.added".into(),
                data: serde_json::json!({"t": "Added", "n": 5}),
                metadata: Value::Object(Default::default()),
                created_at: chrono::Utc::now(),
            },
            RecordedEvent {
                global_position: 2,
                stream_id: "testagg-1".into(),
                stream_version: 2,
                event_type: "test.unknown".into(),
                data: serde_json::json!({"garbage": true}),
                metadata: Value::Object(Default::default()),
                created_at: chrono::Utc::now(),
            },
            RecordedEvent {
                global_position: 3,
                stream_id: "testagg-1".into(),
                stream_version: 3,
                event_type: "test.added".into(),
                data: serde_json::json!({"t": "Added", "n": 3}),
                metadata: Value::Object(Default::default()),
                created_at: chrono::Utc::now(),
            },
        ];

        let root = AggregateRoot::<TestAgg>::hydrate("testagg-1".into(), &events, 3);
        // Only 2 valid events applied
        assert_eq!(root.state.total, 8);
        assert_eq!(root.state.event_count, 2);
    }

    #[test]
    fn hydrate_empty_events() {
        let root = AggregateRoot::<TestAgg>::hydrate("testagg-1".into(), &[], 0);
        assert_eq!(root.state, TestAgg::default());
        assert_eq!(root.version, 0);
    }

    // ---- EventData serde roundtrip ----

    #[test]
    fn event_data_roundtrip_added() {
        let event = TestEvent::Added { n: 42 };
        let json = serde_json::to_value(&event).unwrap();
        let back: TestEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back.event_type(), "test.added");
        if let TestEvent::Added { n } = back {
            assert_eq!(n, 42);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn event_data_roundtrip_cleared() {
        let event = TestEvent::Cleared;
        let json = serde_json::to_value(&event).unwrap();
        let back: TestEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back.event_type(), "test.cleared");
    }

    #[test]
    fn event_data_invalid_json_returns_err() {
        let bad = serde_json::json!({"not_valid": 123});
        assert!(serde_json::from_value::<TestEvent>(bad).is_err());
    }

    // ---- Event wrapper ----

    #[test]
    fn event_new_has_empty_metadata() {
        let e = Event::new(TestEvent::Added { n: 1 });
        assert!(e.metadata.is_object());
        assert_eq!(e.metadata.as_object().unwrap().len(), 0);
    }

    #[test]
    fn event_with_metadata() {
        let meta = serde_json::json!({"user_id": "abc", "ip": "1.2.3.4"});
        let e = Event::new(TestEvent::Added { n: 1 }).with_metadata(meta.clone());
        assert_eq!(e.metadata, meta);
    }

    // ---- StreamQuery ----

    #[test]
    fn stream_query_default() {
        let q = StreamQuery::default();
        assert_eq!(q.direction, ReadDirection::Forward);
        assert_eq!(q.from_version, 0);
        assert_eq!(q.limit, 1000);
    }

    // ---- ExpectedVersion ----

    #[test]
    fn expected_version_equality() {
        assert_eq!(ExpectedVersion::NoStream, ExpectedVersion::NoStream);
        assert_eq!(ExpectedVersion::Exact(5), ExpectedVersion::Exact(5));
        assert_ne!(ExpectedVersion::Exact(5), ExpectedVersion::Exact(6));
        assert_ne!(ExpectedVersion::NoStream, ExpectedVersion::Any);
    }

    #[test]
    fn expected_version_debug() {
        let dbg = format!("{:?}", ExpectedVersion::Exact(42));
        assert!(dbg.contains("42"));
    }

    // ---- Stream category ----

    #[test]
    fn stream_category_is_static() {
        assert_eq!(TestAgg::stream_category().as_str(), "testagg");
    }
}
