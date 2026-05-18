//! User-global config decoder.
//!
//! Pure module — no I/O. Parses the JSON produced by evaluating
//! `~/.config/forest/forest.cue` (via the shell-side `cue_eval` module)
//! into a typed [`UserConfig`].
//!
//! Schema source of truth: `cue/forest-sdk/user_config.cue` and
//! TASKS/018-global-tools.md §1a.4.

use std::collections::BTreeMap;

use crate::global::names::{NameError, validate_tool_name};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserConfig {
    /// Arbitrary string→string kv set by `forest global set`.
    pub user: BTreeMap<String, String>,
    /// Per-tool pins. Key is `<org>/<name>`.
    pub dependencies: BTreeMap<String, Dependency>,
    /// Org-catalogue subscriptions. Key is the organisation name.
    pub org_catalog: BTreeMap<String, OrgCatalog>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub version: String,
    /// Optional client-side shim alias. If `None`, the shim name comes from
    /// the component manifest's `tool.name`.
    pub shim_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgCatalog {
    pub enabled: bool,
    pub banned: Vec<String>,
    /// Per-tool pins inside this catalogue: upstream `tool.name` → version.
    pub pins: BTreeMap<String, String>,
    /// Alias map: upstream `tool.name` → local shim_name.
    pub aliases: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserConfigError {
    InvalidJson(String),
    /// The CUE evaluator emits `{"config": {...}}` — if the outer shape is
    /// wrong, we surface this clearly rather than silently treating the input
    /// as the inner config.
    MissingConfigRoot,
    InvalidDependencyKey(String),
    InvalidVersion(String),
    InvalidShimName(NameError),
    InvalidAliasName(NameError),
}

/// Parse the JSON form of `~/.config/forest/forest.cue` (i.e. the output of
/// `cue eval --out json` applied to that file).
///
/// Expected outer shape: `{ "config": { "user": {...}, "dependencies": {...}, "org_catalog": {...} } }`.
/// Any missing sub-section defaults to empty.
pub fn parse(json: &str) -> Result<UserConfig, UserConfigError> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| UserConfigError::InvalidJson(e.to_string()))?;
    let root = value
        .as_object()
        .ok_or(UserConfigError::InvalidJson("root must be an object".into()))?;
    let cfg = root
        .get("config")
        .and_then(|v| v.as_object())
        .ok_or(UserConfigError::MissingConfigRoot)?;

    let user = match cfg.get("user") {
        None | Some(serde_json::Value::Null) => BTreeMap::new(),
        Some(serde_json::Value::Object(map)) => map
            .iter()
            .map(|(k, v)| {
                let val = v
                    .as_str()
                    .ok_or(UserConfigError::InvalidJson(
                        "user values must be strings".into(),
                    ))?
                    .to_string();
                Ok((k.clone(), val))
            })
            .collect::<Result<BTreeMap<_, _>, UserConfigError>>()?,
        Some(_) => {
            return Err(UserConfigError::InvalidJson(
                "user must be an object".into(),
            ));
        }
    };

    let dependencies = match cfg.get("dependencies") {
        None | Some(serde_json::Value::Null) => BTreeMap::new(),
        Some(serde_json::Value::Object(map)) => {
            let mut out = BTreeMap::new();
            for (k, v) in map {
                if !is_qualified_dep_key(k) {
                    return Err(UserConfigError::InvalidDependencyKey(k.clone()));
                }
                let dep_obj = v.as_object().ok_or_else(|| {
                    UserConfigError::InvalidJson(format!(
                        "dependency {k:?} must be an object"
                    ))
                })?;
                let version = dep_obj
                    .get("version")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| {
                        UserConfigError::InvalidVersion(format!(
                            "dependency {k:?} missing version"
                        ))
                    })?
                    .to_string();
                if !is_semver_shape(&version) {
                    return Err(UserConfigError::InvalidVersion(version));
                }
                let shim_name = match dep_obj.get("shim_name") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(serde_json::Value::String(s)) => {
                        validate_tool_name(s)
                            .map_err(UserConfigError::InvalidShimName)?;
                        Some(s.clone())
                    }
                    Some(_) => {
                        return Err(UserConfigError::InvalidJson(
                            "shim_name must be a string".into(),
                        ));
                    }
                };
                out.insert(k.clone(), Dependency { version, shim_name });
            }
            out
        }
        Some(_) => {
            return Err(UserConfigError::InvalidJson(
                "dependencies must be an object".into(),
            ));
        }
    };

    let org_catalog = match cfg.get("org_catalog") {
        None | Some(serde_json::Value::Null) => BTreeMap::new(),
        Some(serde_json::Value::Object(map)) => {
            let mut out = BTreeMap::new();
            for (org, v) in map {
                let entry = v.as_object().ok_or_else(|| {
                    UserConfigError::InvalidJson(format!(
                        "org_catalog.{org:?} must be an object"
                    ))
                })?;
                let enabled = entry
                    .get("enabled")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(true);
                let banned = match entry.get("banned") {
                    None | Some(serde_json::Value::Null) => Vec::new(),
                    Some(serde_json::Value::Array(arr)) => arr
                        .iter()
                        .map(|x| {
                            x.as_str()
                                .map(str::to_string)
                                .ok_or(UserConfigError::InvalidJson(
                                    "banned[] must be strings".into(),
                                ))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    Some(_) => {
                        return Err(UserConfigError::InvalidJson(
                            "banned must be an array".into(),
                        ));
                    }
                };
                let pins = parse_string_map(entry.get("pins"), "pins", |val| {
                    if is_semver_shape(val) {
                        Ok(val.to_string())
                    } else {
                        Err(UserConfigError::InvalidVersion(val.to_string()))
                    }
                })?;
                let aliases = parse_string_map(entry.get("aliases"), "aliases", |val| {
                    validate_tool_name(val)
                        .map_err(UserConfigError::InvalidAliasName)?;
                    Ok(val.to_string())
                })?;
                out.insert(
                    org.clone(),
                    OrgCatalog {
                        enabled,
                        banned,
                        pins,
                        aliases,
                    },
                );
            }
            out
        }
        Some(_) => {
            return Err(UserConfigError::InvalidJson(
                "org_catalog must be an object".into(),
            ));
        }
    };

    Ok(UserConfig {
        user,
        dependencies,
        org_catalog,
    })
}

fn is_qualified_dep_key(s: &str) -> bool {
    match s.split_once('/') {
        None => false,
        Some((org, name)) => !org.is_empty() && !name.is_empty() && !name.contains('/'),
    }
}

fn is_semver_shape(s: &str) -> bool {
    // Minimal semver-shape check: three dot-separated non-empty ASCII-digit
    // groups, optionally followed by `-<pre>` or `+<build>` segments.
    // Mirrors the CUE regex `^\d+\.\d+\.\d+`.
    let core = s.split(['-', '+']).next().unwrap_or(s);
    let parts: Vec<_> = core.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    parts
        .iter()
        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

fn parse_string_map<F>(
    value: Option<&serde_json::Value>,
    field: &str,
    mut validate_value: F,
) -> Result<BTreeMap<String, String>, UserConfigError>
where
    F: FnMut(&str) -> Result<String, UserConfigError>,
{
    match value {
        None | Some(serde_json::Value::Null) => Ok(BTreeMap::new()),
        Some(serde_json::Value::Object(map)) => {
            let mut out = BTreeMap::new();
            for (k, v) in map {
                let raw = v.as_str().ok_or(UserConfigError::InvalidJson(format!(
                    "{field} values must be strings"
                )))?;
                let val = validate_value(raw)?;
                out.insert(k.clone(), val);
            }
            Ok(out)
        }
        Some(_) => Err(UserConfigError::InvalidJson(format!(
            "{field} must be an object"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Happy paths --------------------------------------------------------

    #[test]
    fn parses_empty_config() {
        let json = r#"{"config": {}}"#;
        let c = parse(json).unwrap();
        assert!(c.user.is_empty());
        assert!(c.dependencies.is_empty());
        assert!(c.org_catalog.is_empty());
    }

    #[test]
    fn parses_minimal_dependency() {
        let json = r#"{
            "config": {
                "dependencies": {
                    "cuteorg/ripgrep": {"version": "14.1.1"}
                }
            }
        }"#;
        let c = parse(json).unwrap();
        assert_eq!(c.dependencies.len(), 1);
        let dep = c.dependencies.get("cuteorg/ripgrep").unwrap();
        assert_eq!(dep.version, "14.1.1");
        assert!(dep.shim_name.is_none());
    }

    #[test]
    fn parses_dependency_with_shim_name_alias() {
        let json = r#"{
            "config": {
                "dependencies": {
                    "cuteorg/ripgrep": {"version": "14.1.1", "shim_name": "rg"}
                }
            }
        }"#;
        let c = parse(json).unwrap();
        let dep = c.dependencies.get("cuteorg/ripgrep").unwrap();
        assert_eq!(dep.shim_name.as_deref(), Some("rg"));
    }

    #[test]
    fn parses_user_kv_section() {
        let json = r#"{
            "config": {
                "user": {"author": "alice@example.com", "favourite_color": "green"}
            }
        }"#;
        let c = parse(json).unwrap();
        assert_eq!(c.user.get("author").unwrap(), "alice@example.com");
        assert_eq!(c.user.get("favourite_color").unwrap(), "green");
    }

    #[test]
    fn parses_full_org_catalog_subscription() {
        // Mirrors examples/global-tools/user-config/forest.cue.
        let json = r#"{
            "config": {
                "dependencies": {
                    "cuteorg/ripgrep": {"version": "14.1.1"}
                },
                "org_catalog": {
                    "cuteorg": {
                        "enabled": true,
                        "banned": ["forest-greet"],
                        "pins": {"myscaffolder": "0.1.0"},
                        "aliases": {"forest-hello": "hello"}
                    }
                }
            }
        }"#;
        let c = parse(json).unwrap();
        let cat = c.org_catalog.get("cuteorg").unwrap();
        assert!(cat.enabled);
        assert_eq!(cat.banned, vec!["forest-greet".to_string()]);
        assert_eq!(cat.pins.get("myscaffolder").unwrap(), "0.1.0");
        assert_eq!(cat.aliases.get("forest-hello").unwrap(), "hello");
    }

    #[test]
    fn parses_catalog_with_default_enabled_true() {
        // `enabled: bool | *true` in the CUE schema — if CUE evaluates and
        // emits the default, the JSON contains `"enabled": true`. We don't
        // tolerate a missing `enabled` field (CUE will always emit it), but
        // we DO tolerate missing `banned`/`pins`/`aliases` (empty by default).
        let json = r#"{
            "config": {
                "org_catalog": {
                    "cuteorg": {"enabled": true}
                }
            }
        }"#;
        let c = parse(json).unwrap();
        let cat = c.org_catalog.get("cuteorg").unwrap();
        assert!(cat.banned.is_empty());
        assert!(cat.pins.is_empty());
        assert!(cat.aliases.is_empty());
    }

    #[test]
    fn parses_disabled_catalog_subscription() {
        // `enabled: false` is a valid state (subscription preserved for
        // historical reasons but inactive).
        let json = r#"{
            "config": {
                "org_catalog": {
                    "cuteorg": {"enabled": false}
                }
            }
        }"#;
        let c = parse(json).unwrap();
        assert!(!c.org_catalog.get("cuteorg").unwrap().enabled);
    }

    // --- Negative paths -----------------------------------------------------

    #[test]
    fn rejects_invalid_json() {
        let err = parse("{not json").unwrap_err();
        let is_invalid = matches!(err, UserConfigError::InvalidJson(_));
        assert!(is_invalid, "got {err:?}");
    }

    #[test]
    fn rejects_missing_config_root() {
        let json = r#"{"dependencies": {}}"#;
        let err = parse(json).unwrap_err();
        assert_eq!(err, UserConfigError::MissingConfigRoot);
    }

    #[test]
    fn rejects_dependency_key_without_org_slash_name() {
        let json = r#"{
            "config": {
                "dependencies": {
                    "ripgrep": {"version": "14.1.1"}
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        let is_key = matches!(err, UserConfigError::InvalidDependencyKey(ref s) if s == "ripgrep");
        assert!(is_key, "got {err:?}");
    }

    #[test]
    fn rejects_invalid_shim_name() {
        // shim_name must be a valid tool name (same regex).
        let json = r#"{
            "config": {
                "dependencies": {
                    "cuteorg/ripgrep": {"version": "14.1.1", "shim_name": "1bad"}
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        let is_shim = matches!(err, UserConfigError::InvalidShimName(_));
        assert!(is_shim, "got {err:?}");
    }

    #[test]
    fn rejects_invalid_alias_target_name() {
        // `aliases: {upstream → mine}` — mine must validate.
        let json = r#"{
            "config": {
                "org_catalog": {
                    "cuteorg": {"enabled": true, "aliases": {"forest-hello": "1bad"}}
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        let is_alias = matches!(err, UserConfigError::InvalidAliasName(_));
        assert!(is_alias, "got {err:?}");
    }

    #[test]
    fn rejects_invalid_version_string() {
        // The CUE schema enforces semver shape, but a defensive client check
        // shields against a hand-edited forest.cue that slipped through.
        let json = r#"{
            "config": {
                "dependencies": {
                    "cuteorg/ripgrep": {"version": "not-a-version"}
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        let is_ver = matches!(err, UserConfigError::InvalidVersion(_));
        assert!(is_ver, "got {err:?}");
    }
}
