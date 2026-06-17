//! Canonical manifest hashing (TASKS/024 — published-version immutability).
//!
//! `manifest_hash` is the content identity of a published version: two publishes
//! of the same version are "identical" iff their canonical manifest hashes match.
//! The manifest embeds every platform's binary `sha256`, so hashing the manifest
//! pins the binaries transitively.
//!
//! Canonicalisation re-serialises the JSON with **lexicographically sorted
//! object keys** and no insignificant whitespace, so an honest re-publish whose
//! CLI reformats or reorders the JSON still hashes identically. The
//! canonicaliser walks the value explicitly rather than relying on
//! `serde_json::Map`'s ordering, so it is correct even if some crate in the
//! build enables serde_json's `preserve_order` feature (feature unification).
//!
//! Pure — no I/O. Nothing calls this yet; it is the groundwork the immutability
//! enforcement (TASKS/024) will build on.

use sha2::{Digest, Sha256};

use crate::ManifestError;

/// Canonical JSON form of a manifest string: sorted object keys, minimal
/// whitespace, array order preserved (arrays are semantically ordered).
pub fn canonical_json(json: &str) -> Result<String, ManifestError> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| ManifestError::InvalidJson(e.to_string()))?;
    Ok(canonicalize(&value))
}

/// `sha256(canonical_json(json))`, hex-encoded. The content identity of a
/// published version (TASKS/024).
pub fn manifest_hash(json: &str) -> Result<String, ManifestError> {
    let canon = canonical_json(json)?;
    Ok(hex::encode(Sha256::digest(canon.as_bytes())))
}

fn canonicalize(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            let mut out = String::from("{");
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                // `to_string` on a string Value produces a correctly-escaped
                // JSON string literal for the key.
                out.push_str(&serde_json::Value::String((*k).clone()).to_string());
                out.push(':');
                out.push_str(&canonicalize(&map[*k]));
            }
            out.push('}');
            out
        }
        serde_json::Value::Array(arr) => {
            let mut out = String::from("[");
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&canonicalize(v));
            }
            out.push(']');
            out
        }
        // Scalars (null/bool/number/string): serde's own compact form.
        scalar => scalar.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = r#"{"kind":"binary","include":{"env":{"B":"2","A":"1"}},"platforms":{"darwin_arm64":{"sha256":"abc","size":10}}}"#;

    #[test]
    fn key_order_does_not_change_hash() {
        let reordered = r#"{"platforms":{"darwin_arm64":{"size":10,"sha256":"abc"}},"include":{"env":{"A":"1","B":"2"}},"kind":"binary"}"#;
        assert_eq!(manifest_hash(A).unwrap(), manifest_hash(reordered).unwrap());
    }

    #[test]
    fn whitespace_does_not_change_hash() {
        let spaced = r#"{
            "kind": "binary",
            "include": { "env": { "A": "1", "B": "2" } },
            "platforms": { "darwin_arm64": { "sha256": "abc", "size": 10 } }
        }"#;
        assert_eq!(manifest_hash(A).unwrap(), manifest_hash(spaced).unwrap());
    }

    #[test]
    fn different_content_changes_hash() {
        let changed = A.replace("\"1\"", "\"99\"");
        assert_ne!(manifest_hash(A).unwrap(), manifest_hash(&changed).unwrap());
    }

    #[test]
    fn array_order_is_significant() {
        let a = r#"{"x":[1,2,3]}"#;
        let b = r#"{"x":[3,2,1]}"#;
        assert_ne!(manifest_hash(a).unwrap(), manifest_hash(b).unwrap());
    }

    #[test]
    fn canonical_json_is_idempotent() {
        let once = canonical_json(A).unwrap();
        let twice = canonical_json(&once).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn hash_is_64_hex_chars() {
        let h = manifest_hash(A).unwrap();
        assert_eq!(h.len(), 64);
        assert!(h.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn invalid_json_errors() {
        assert!(matches!(
            manifest_hash("{not json"),
            Err(ManifestError::InvalidJson(_))
        ));
    }
}
