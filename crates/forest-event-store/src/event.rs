use chrono::{DateTime, Utc};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

/// Trait that domain event enums must implement.
///
/// Serialization is handled automatically via serde bounds —
/// implementors only need to provide the event type tag.
pub trait EventData: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Unique event type string (e.g. "order.created", "order.item_added").
    fn event_type(&self) -> &'static str;
}

/// An event to be appended (before persistence).
pub struct Event<E: EventData> {
    pub event: E,
    pub metadata: Value,
}

impl<E: EventData> Event<E> {
    pub fn new(event: E) -> Self {
        Self {
            event,
            metadata: Value::Object(serde_json::Map::new()),
        }
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }
}

/// A persisted event read back from the store.
#[derive(Debug, Clone)]
pub struct RecordedEvent {
    pub global_position: i64,
    pub stream_id: String,
    pub stream_version: i64,
    pub event_type: String,
    pub data: Value,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}
