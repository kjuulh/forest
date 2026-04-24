//! Hollow acceptance tests. The library entry point exists only so the crate
//! has somewhere to put shared scaffolding (e.g. lazy harness initialization);
//! actual scenarios live in `tests/`.

use std::sync::OnceLock;

use hollow_test_harness::Harness;

/// Shared harness across all tests in this crate. Building artifacts and
/// bootstrapping the remote host is expensive — we do it once per `cargo test`
/// invocation. Returns `None` when `HOLLOW_TEST_HOST` is unset; tests treat
/// that as a skip.
pub fn harness() -> Option<&'static Harness> {
    static ONCE: OnceLock<Option<Harness>> = OnceLock::new();
    ONCE.get_or_init(Harness::from_env).as_ref()
}

/// Print a uniform skip message and return early. Use at the top of each test
/// when `harness()` returns `None`.
#[macro_export]
macro_rules! skip_unless_harness {
    () => {
        match $crate::harness() {
            Some(h) => h,
            None => {
                eprintln!(
                    "[hollow-acceptance] skipping: HOLLOW_TEST_HOST unset — set it to a KVM-capable Linux host (e.g. user@host or an SSH alias)"
                );
                return Ok(());
            }
        }
    };
}
