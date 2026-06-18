//! `forest-contrib/build-rust` — build component (DATA-312).
//!
//! Thin wrapper: drives the forest component protocol and delegates
//! `commands/build` to `forest-build-core`, which reads the project manifest
//! and runs `cargo`. Reports its required tools via `_meta/describe` so forest
//! verifies them before dispatch.

fn main() {
    forest_build_core::component::serve(forest_build_core::Toolchain::Rust);
}
