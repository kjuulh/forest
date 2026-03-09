#![no_main]

use arbitrary::Arbitrary;
use forest_event_store::{Aggregate, AggregateRoot, EventData, IntoStreamCategory, RecordedEvent, StreamCategory};
use libfuzzer_sys::fuzz_target;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Default)]
struct Acc {
    sum: i64,
    count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Arbitrary)]
enum AccEvent {
    Add(i64),
    Reset,
}

impl EventData for AccEvent {
    fn event_type(&self) -> &'static str {
        match self {
            AccEvent::Add(_) => "acc.add",
            AccEvent::Reset => "acc.reset",
        }
    }

}

impl Aggregate for Acc {
    type Event = AccEvent;

    fn stream_category() -> StreamCategory {
        "fuzzacc".into_stream_category()
    }

    fn apply(&mut self, event: &AccEvent) {
        self.count += 1;
        match event {
            AccEvent::Add(n) => self.sum = self.sum.saturating_add(*n),
            AccEvent::Reset => self.sum = 0,
        }
    }
}

#[derive(Debug, Arbitrary)]
struct HydrateInput {
    events: Vec<AccEvent>,
}

fuzz_target!(|input: HydrateInput| {
    if input.events.is_empty() {
        return;
    }

    // Build RecordedEvents (simulating what DB would return)
    let recorded: Vec<RecordedEvent> = input
        .events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let json = serde_json::to_value(e).ok()?;
            Some(RecordedEvent {
                global_position: (i + 1) as i64,
                stream_id: "fuzzacc-hydrate".into(),
                stream_version: (i + 1) as i64,
                event_type: e.event_type().into(),
                data: json,
                metadata: Value::Object(Default::default()),
                created_at: chrono::Utc::now(),
            })
        })
        .collect();

    // Hydrate
    let root =
        AggregateRoot::<Acc>::hydrate("fuzzacc-hydrate".into(), &recorded, recorded.len() as i64);

    // Direct fold
    let mut direct = Acc::default();
    for e in &input.events {
        // Only count events that serialize successfully (matching hydrate filter)
        if serde_json::to_value(e).is_ok() {
            direct.apply(e);
        }
    }

    assert_eq!(root.state.sum, direct.sum);
    assert_eq!(root.state.count, direct.count);
    assert_eq!(root.version, recorded.len() as i64);
    assert!(!root.has_pending());
});
