// forest-hello — a TOOL_BINARY example for the global-tools demo.
//
// This is a plain CLI binary. Forest invokes it via argv passthrough — there
// is no `_meta/describe`, no method dispatch, no component protocol. The
// `forest.component.cue` next to this crate declares only a #Tool facet,
// which is what makes the registry classify it as shape=TOOL_BINARY.

fn main() {
    let who = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "anonymous".to_string());
    println!("hello, {who}!");
}
