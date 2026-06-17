//! Pure resolution of the env vars to inject when running a global tool.
//!
//! TASKS/023 §B6/B7. Given a component's declared default env (from the
//! manifest's `include.env`, cached beside the binary), a developer's per-tool
//! local override (from `~/.config/forest/forest.cue`), and the set of env-var
//! names already present in the ambient process environment, compute exactly
//! the `(key, value)` pairs forest should set on the child.
//!
//! Precedence, lowest → highest:
//!   1. component-declared env (manifest)
//!   2. per-tool local override (user forest.cue)
//!   3. ambient process environment (never overwritten)
//!
//! Pure core — no I/O. The effectful caller builds `ambient` from
//! `std::env::vars()` and applies the result via `Command::env`.

use std::collections::{BTreeMap, BTreeSet};

/// Compute the env vars to set on the child process.
///
/// `component` and `local` are the declared defaults and the local override.
/// `ambient` is the set of env-var names already present in the process
/// environment; any key in it is omitted from the result so the inherited
/// value wins (the ambient shell environment always wins).
pub fn resolve_injection(
    component: &BTreeMap<String, String>,
    local: &BTreeMap<String, String>,
    ambient: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut out = component.clone();
    // Local override wins over the component default.
    for (k, v) in local {
        out.insert(k.clone(), v.clone());
    }
    // Ambient always wins: never set a key the environment already carries.
    out.retain(|k, _| !ambient.contains(k));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn component_default_applied_when_absent() {
        let out = resolve_injection(
            &map(&[("FUNGUS_SERVER", "https://prod")]),
            &map(&[]),
            &set(&[]),
        );
        assert_eq!(out.get("FUNGUS_SERVER").unwrap(), "https://prod");
    }

    #[test]
    fn ambient_wins_over_component_and_local() {
        // FUNGUS_SERVER is present in the ambient env → never injected,
        // regardless of component/local defaults.
        let out = resolve_injection(
            &map(&[("FUNGUS_SERVER", "https://prod")]),
            &map(&[("FUNGUS_SERVER", "http://localhost")]),
            &set(&["FUNGUS_SERVER"]),
        );
        assert!(!out.contains_key("FUNGUS_SERVER"));
    }

    #[test]
    fn local_overrides_component_when_not_ambient() {
        let out = resolve_injection(
            &map(&[("FUNGUS_SERVER", "https://prod")]),
            &map(&[("FUNGUS_SERVER", "http://localhost")]),
            &set(&[]),
        );
        assert_eq!(out.get("FUNGUS_SERVER").unwrap(), "http://localhost");
    }

    #[test]
    fn local_can_add_new_key() {
        let out = resolve_injection(&map(&[]), &map(&[("EXTRA", "1")]), &set(&[]));
        assert_eq!(out.get("EXTRA").unwrap(), "1");
    }

    #[test]
    fn local_empty_value_is_injected_when_not_ambient() {
        let out = resolve_injection(&map(&[]), &map(&[("EMPTY", "")]), &set(&[]));
        assert_eq!(out.get("EMPTY").unwrap(), "");
    }

    #[test]
    fn empty_inputs_yield_empty() {
        assert!(resolve_injection(&map(&[]), &map(&[]), &set(&[])).is_empty());
    }

    proptest! {
        /// P1 (ambient-wins): no key present in `ambient` appears in the result.
        #[test]
        fn p1_ambient_keys_never_injected(
            comp in proptest::collection::btree_map("[A-Z]{1,6}", "[a-z]{0,6}", 0..6),
            local in proptest::collection::btree_map("[A-Z]{1,6}", "[a-z]{0,6}", 0..6),
            ambient_vec in proptest::collection::vec("[A-Z]{1,6}", 0..6),
        ) {
            let ambient: BTreeSet<String> = ambient_vec.into_iter().collect();
            let out = resolve_injection(&comp, &local, &ambient);
            for k in out.keys() {
                prop_assert!(!ambient.contains(k), "ambient key {k:?} leaked into result");
            }
        }

        /// P2 (precedence) + P3 (no-spurious-keys): every result key comes from
        /// component ∪ local; local value wins when the key is in both and not
        /// ambient.
        #[test]
        fn p2_precedence_and_provenance(
            comp in proptest::collection::btree_map("[A-Z]{1,6}", "c[a-z]{0,4}", 0..6),
            local in proptest::collection::btree_map("[A-Z]{1,6}", "l[a-z]{0,4}", 0..6),
            ambient_vec in proptest::collection::vec("[A-Z]{1,6}", 0..6),
        ) {
            let ambient: BTreeSet<String> = ambient_vec.into_iter().collect();
            let out = resolve_injection(&comp, &local, &ambient);
            for (k, v) in &out {
                prop_assert!(comp.contains_key(k) || local.contains_key(k));
                let expected = local.get(k).or_else(|| comp.get(k)).unwrap();
                prop_assert_eq!(v, expected);
            }
        }

        /// P4 (idempotence): re-resolving with the result as both inputs and the
        /// same ambient is stable.
        #[test]
        fn p4_idempotent(
            comp in proptest::collection::btree_map("[A-Z]{1,6}", "[a-z]{0,6}", 0..6),
            local in proptest::collection::btree_map("[A-Z]{1,6}", "[a-z]{0,6}", 0..6),
            ambient_vec in proptest::collection::vec("[A-Z]{1,6}", 0..6),
        ) {
            let ambient: BTreeSet<String> = ambient_vec.into_iter().collect();
            let once = resolve_injection(&comp, &local, &ambient);
            let twice = resolve_injection(&once, &once, &ambient);
            prop_assert_eq!(once, twice);
        }
    }
}
