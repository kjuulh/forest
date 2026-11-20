use sha2::{Digest, Sha256};

use crate::ManifestError;

pub fn canonical_json(json: &str) -> Result<String, ManifestError> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| ManifestError::InvalidJson(e.to_string()))?;
    Ok(canonicalize(&value))
}

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
