#![no_main]

use arbitrary::Arbitrary;
use forest_event_store::{Aggregate, AggregateRoot, EventData, IntoStreamCategory, StreamCategory};
use libfuzzer_sys::fuzz_target;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default)]
struct FuzzCounter {
    value: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Arbitrary)]
enum FuzzCounterEvent {
    Add(i64),
    Sub(i64),
    Mul(i32),
    Zero,
}

impl EventData for FuzzCounterEvent {
    fn event_type(&self) -> &'static str {
        match self {
            FuzzCounterEvent::Add(_) => "fc.add",
            FuzzCounterEvent::Sub(_) => "fc.sub",
            FuzzCounterEvent::Mul(_) => "fc.mul",
            FuzzCounterEvent::Zero => "fc.zero",
        }
    }

}

impl Aggregate for FuzzCounter {
    type Event = FuzzCounterEvent;

    fn stream_category() -> StreamCategory {
        "fuzzcounter".into_stream_category()
    }

    fn apply(&mut self, event: &FuzzCounterEvent) {
        match event {
            FuzzCounterEvent::Add(n) => self.value = self.value.saturating_add(*n),
            FuzzCounterEvent::Sub(n) => self.value = self.value.saturating_sub(*n),
            FuzzCounterEvent::Mul(n) => self.value = self.value.saturating_mul(*n as i64),
            FuzzCounterEvent::Zero => self.value = 0,
        }
    }
}

#[derive(Debug, Arbitrary)]
struct FuzzInput {
    events: Vec<FuzzCounterEvent>,
}

fuzz_target!(|input: FuzzInput| {
    if input.events.is_empty() {
        return;
    }

    // Fold via AggregateRoot
    let mut root = AggregateRoot::<FuzzCounter>::new("fuzzcounter-fuzz".into());
    for e in &input.events {
        root.record(e.clone());
    }

    // Fold directly
    let mut direct = FuzzCounter::default();
    for e in &input.events {
        direct.apply(e);
    }

    // Must match
    assert_eq!(root.state.value, direct.value);
    assert_eq!(root.pending_count(), input.events.len());

    // take_pending must not alter state
    let state_before = root.state.value;
    let taken = root.take_pending();
    assert_eq!(taken.len(), input.events.len());
    assert_eq!(root.state.value, state_before);
    assert!(!root.has_pending());
});
