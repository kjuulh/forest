#![no_main]

use arbitrary::Arbitrary;
use forest_event_store::EventData;
use libfuzzer_sys::fuzz_target;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Arbitrary)]
enum FuzzEvent {
    A { val: i64 },
    B { name: String, count: u32 },
    C,
}

impl EventData for FuzzEvent {
    fn event_type(&self) -> &'static str {
        match self {
            FuzzEvent::A { .. } => "fuzz.a",
            FuzzEvent::B { .. } => "fuzz.b",
            FuzzEvent::C => "fuzz.c",
        }
    }

}

fuzz_target!(|event: FuzzEvent| {
    // Roundtrip: serialize then deserialize must produce the same event_type
    let json = serde_json::to_value(&event).unwrap();
    let back: FuzzEvent = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(event.event_type(), back.event_type());

    // Re-serialize must produce identical JSON
    let json2 = serde_json::to_value(&back).unwrap();
    assert_eq!(json, json2);
});
