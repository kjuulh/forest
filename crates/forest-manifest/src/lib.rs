//! Manifest JSON decoder + shape derivation.
//!
//! Shared between the forest CLI client and the forest server. Pure — no I/O,
//! no async, no thread-local state. Parses the manifest JSON blob (the body of
//! `GetComponentManifest`) into a typed [`Manifest`] and computes its
//! [`ComponentShape`] per TASKS/018-global-tools.md §1a.2e.
//!
//! The server uses [`parse`] at `publish_manifest` time to enforce rules 1–7
//! from §1a.2; the client uses [`parse`] at runtime as defence-in-depth.

#![doc(html_no_source)]

use std::collections::BTreeMap;

pub mod names;

use names::{NameError, validate_tool_name};

// --- Public types ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub kind: ManifestKind,
    pub tool: Option<ToolFacet>,
    pub methods: Vec<String>,
    pub platforms: BTreeMap<PlatformKey, Platform>,
    /// Derived from `(kind, tool, methods)` at parse time. Always consistent
    /// with the other fields; consumers should rely on `shape` rather than
    /// re-deriving.
    pub shape: ComponentShape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestKind {
    Binary,
    External,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolFacet {
    pub name: String,
    pub argv_passthrough: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Os {
    Linux,
    Darwin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Arch {
    Amd64,
    Arm64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlatformKey {
    pub os: Os,
    pub arch: Arch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archive {
    None,
    TarGz,
    TarXz,
    TarZst,
    Zip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub sha256: String,
    pub size: Option<u64>,
    // External-only fields. For `kind: binary` these must all be None at parse.
    pub url: Option<String>,
    pub archive: Archive,
    pub binary_in_archive: Option<String>,
    pub archive_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentShape {
    Component,
    HybridComponent,
    ToolBinary,
    ToolExternal,
}

// --- Errors ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    InvalidJson(String),
    MissingKind,
    UnknownKind(String),
    /// §1a.2 rule 2: external manifests MUST declare a tool facet.
    ExternalRequiresTool,
    /// §1a.2 rule 7: a binary manifest needs methods OR a tool.
    BinaryRequiresMethodsOrTool,
    /// External manifests have no describe protocol, hence no methods.
    ExternalCannotDeclareMethods,
    InvalidToolName(NameError),
    InvalidArgvPassthrough,
    InvalidPlatformKey(String),
    UnsupportedOs(String),
    UnsupportedArch(String),
    UnsupportedArchive(String),
    InvalidSha256(String),
    InvalidArchiveSha256(String),
    InvalidUrl {
        url: String,
        reason: &'static str,
    },
    BinaryKindForbidsField(&'static str),
    ExternalKindRequires(&'static str),
    ArchiveRequiresBinaryInArchive,
    InvalidBinaryInArchive(&'static str),
}

// --- Public API -----------------------------------------------------------

/// Parse a manifest JSON blob (as served by `GetComponentManifest`) into a
/// validated typed form.
///
/// Total over any input string — never panics. Returns the first invariant
/// violation as a `ManifestError`.
pub fn parse(json: &str) -> Result<Manifest, ManifestError> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| ManifestError::InvalidJson(e.to_string()))?;
    let obj = value.as_object().ok_or_else(|| {
        ManifestError::InvalidJson("manifest root must be a JSON object".into())
    })?;

    // --- kind ---------------------------------------------------------
    let kind = parse_kind(obj.get("kind"))?;

    // --- tool ---------------------------------------------------------
    let tool = obj
        .get("tool")
        .filter(|v| !v.is_null())
        .map(parse_tool_facet)
        .transpose()?;

    // --- methods ------------------------------------------------------
    let methods = match obj.get("methods") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or(ManifestError::InvalidJson(
                        "methods[] must be strings".into(),
                    ))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(ManifestError::InvalidJson(
                "methods must be an array of strings".into(),
            ));
        }
    };

    // --- shape derivation runs BEFORE platform parsing so we catch
    //     "wrong shape" before "wrong sha". §1a.2e.
    let shape = derive_shape(kind, tool.is_some(), !methods.is_empty())?;

    // --- platforms ----------------------------------------------------
    let platforms_obj = obj
        .get("platforms")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            ManifestError::InvalidJson("platforms must be a JSON object".into())
        })?;

    let mut platforms = BTreeMap::new();
    for (key, raw) in platforms_obj {
        let pk = parse_platform_key(key)?;
        let platform = parse_platform(raw, kind)?;
        platforms.insert(pk, platform);
    }

    Ok(Manifest {
        kind,
        tool,
        methods,
        platforms,
        shape,
    })
}

/// Pure derivation of the shape from the three discriminator inputs.
/// Exposed separately so the resolver and the server-side validator can
/// share a single source of truth (§1a.2e).
pub fn derive_shape(
    kind: ManifestKind,
    has_tool: bool,
    has_methods: bool,
) -> Result<ComponentShape, ManifestError> {
    match (kind, has_tool, has_methods) {
        // Binary kind.
        (ManifestKind::Binary, false, false) => Err(ManifestError::BinaryRequiresMethodsOrTool),
        (ManifestKind::Binary, false, true) => Ok(ComponentShape::Component),
        (ManifestKind::Binary, true, false) => Ok(ComponentShape::ToolBinary),
        (ManifestKind::Binary, true, true) => Ok(ComponentShape::HybridComponent),

        // External kind. Rule 2 takes precedence over rule "external + methods"
        // because "external must have a tool" is the more specific spec sentence.
        (ManifestKind::External, false, _) => Err(ManifestError::ExternalRequiresTool),
        (ManifestKind::External, true, true) => Err(ManifestError::ExternalCannotDeclareMethods),
        (ManifestKind::External, true, false) => Ok(ComponentShape::ToolExternal),
    }
}

// --- Internal parsers -----------------------------------------------------

fn parse_kind(v: Option<&serde_json::Value>) -> Result<ManifestKind, ManifestError> {
    match v.and_then(|v| v.as_str()) {
        None => Err(ManifestError::MissingKind),
        Some("binary") => Ok(ManifestKind::Binary),
        Some("external") => Ok(ManifestKind::External),
        Some(other) => Err(ManifestError::UnknownKind(other.to_string())),
    }
}

fn parse_tool_facet(v: &serde_json::Value) -> Result<ToolFacet, ManifestError> {
    let obj = v.as_object().ok_or_else(|| {
        ManifestError::InvalidJson("tool must be an object".into())
    })?;

    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ManifestError::InvalidJson("tool.name missing".into()))?
        .to_string();
    validate_tool_name(&name).map_err(ManifestError::InvalidToolName)?;

    let argv_passthrough = obj
        .get("argv_passthrough")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if !argv_passthrough {
        return Err(ManifestError::InvalidArgvPassthrough);
    }

    let description = obj
        .get("description")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    Ok(ToolFacet {
        name,
        argv_passthrough,
        description,
    })
}

fn parse_platform_key(key: &str) -> Result<PlatformKey, ManifestError> {
    let (os_str, arch_str) = key
        .split_once('_')
        .ok_or_else(|| ManifestError::InvalidPlatformKey(key.to_string()))?;
    let os = match os_str {
        "linux" => Os::Linux,
        "darwin" => Os::Darwin,
        other => return Err(ManifestError::UnsupportedOs(other.to_string())),
    };
    let arch = match arch_str {
        "amd64" => Arch::Amd64,
        "arm64" => Arch::Arm64,
        other => return Err(ManifestError::UnsupportedArch(other.to_string())),
    };
    Ok(PlatformKey { os, arch })
}

fn parse_platform(
    raw: &serde_json::Value,
    kind: ManifestKind,
) -> Result<Platform, ManifestError> {
    let obj = raw.as_object().ok_or_else(|| {
        ManifestError::InvalidJson("platform entry must be an object".into())
    })?;

    let sha256 = obj
        .get("sha256")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ManifestError::InvalidSha256("missing".into()))?
        .to_string();
    if !is_sha256_hex(&sha256) {
        return Err(ManifestError::InvalidSha256(sha256));
    }

    let size = obj.get("size").and_then(|v| v.as_u64());

    let url = optional_string(obj, "url")?;
    let archive = match obj.get("archive").and_then(|v| v.as_str()) {
        None | Some("none") => Archive::None,
        Some("tar.gz") => Archive::TarGz,
        Some("tar.xz") => Archive::TarXz,
        Some("tar.zst") => Archive::TarZst,
        Some("zip") => Archive::Zip,
        Some(other) => return Err(ManifestError::UnsupportedArchive(other.to_string())),
    };
    let binary_in_archive = optional_string(obj, "binary_in_archive")?;
    let archive_sha256 = optional_string(obj, "archive_sha256")?;
    if let Some(ref s) = archive_sha256
        && !is_sha256_hex(s)
    {
        return Err(ManifestError::InvalidArchiveSha256(s.clone()));
    }

    // kind-specific invariants
    match kind {
        ManifestKind::Binary => {
            if url.is_some() {
                return Err(ManifestError::BinaryKindForbidsField("url"));
            }
            if !matches!(archive, Archive::None) {
                return Err(ManifestError::BinaryKindForbidsField("archive"));
            }
            if binary_in_archive.is_some() {
                return Err(ManifestError::BinaryKindForbidsField("binary_in_archive"));
            }
            if archive_sha256.is_some() {
                return Err(ManifestError::BinaryKindForbidsField("archive_sha256"));
            }
        }
        ManifestKind::External => {
            let url_str = url
                .as_deref()
                .ok_or(ManifestError::ExternalKindRequires("url"))?;
            validate_external_url(url_str)?;
            // archive ≠ "none" ⇒ binary_in_archive required.
            if !matches!(archive, Archive::None) && binary_in_archive.is_none() {
                return Err(ManifestError::ArchiveRequiresBinaryInArchive);
            }
            if let Some(path) = binary_in_archive.as_deref() {
                validate_binary_in_archive(path)?;
            }
        }
    }

    Ok(Platform {
        sha256,
        size,
        url,
        archive,
        binary_in_archive,
        archive_sha256,
    })
}

fn optional_string(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
) -> Result<Option<String>, ManifestError> {
    match obj.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(ManifestError::InvalidJson(format!(
            "{field} must be a string"
        ))),
    }
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn validate_external_url(url: &str) -> Result<(), ManifestError> {
    if !url.starts_with("https://") {
        return Err(ManifestError::InvalidUrl {
            url: url.to_string(),
            reason: "scheme must be https",
        });
    }
    Ok(())
}

/// §1a.2d (subset enforced at the manifest layer; the full algorithm
/// lives in `crate::global::extract::canonicalise()` when that module
/// lands — for now we enforce the high-value defences).
fn validate_binary_in_archive(path: &str) -> Result<(), ManifestError> {
    if path.is_empty() {
        return Err(ManifestError::InvalidBinaryInArchive("empty"));
    }
    if path.len() > 256 {
        return Err(ManifestError::InvalidBinaryInArchive("too long"));
    }
    if path.starts_with('/') {
        return Err(ManifestError::InvalidBinaryInArchive("absolute path"));
    }
    if path.starts_with('~') {
        return Err(ManifestError::InvalidBinaryInArchive("home-expansion"));
    }
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err(ManifestError::InvalidBinaryInArchive("empty segment"));
        }
        if segment == "." || segment == ".." {
            return Err(ManifestError::InvalidBinaryInArchive(
                "dot / dotdot segment",
            ));
        }
        if segment.starts_with('.') {
            return Err(ManifestError::InvalidBinaryInArchive("hidden segment"));
        }
        if segment
            .bytes()
            .any(|b| b == 0 || b == b'\r' || b == b'\n' || b == b'\\')
        {
            return Err(ManifestError::InvalidBinaryInArchive(
                "forbidden byte (NUL/CR/LF/backslash)",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- derive_shape: the full 2x2x2 matrix --------------------------------

    #[test]
    fn shape_binary_methods_only_is_component() {
        let s = derive_shape(ManifestKind::Binary, false, true).unwrap();
        assert_eq!(s, ComponentShape::Component);
    }

    #[test]
    fn shape_binary_methods_plus_tool_is_hybrid() {
        let s = derive_shape(ManifestKind::Binary, true, true).unwrap();
        assert_eq!(s, ComponentShape::HybridComponent);
    }

    #[test]
    fn shape_binary_tool_only_is_tool_binary() {
        let s = derive_shape(ManifestKind::Binary, true, false).unwrap();
        assert_eq!(s, ComponentShape::ToolBinary);
    }

    #[test]
    fn shape_binary_no_tool_no_methods_is_rule7_violation() {
        // §1a.2 rule 7: binary needs methods OR tool.
        let err = derive_shape(ManifestKind::Binary, false, false).unwrap_err();
        assert_eq!(err, ManifestError::BinaryRequiresMethodsOrTool);
    }

    #[test]
    fn shape_external_tool_only_is_tool_external() {
        let s = derive_shape(ManifestKind::External, true, false).unwrap();
        assert_eq!(s, ComponentShape::ToolExternal);
    }

    #[test]
    fn shape_external_no_tool_is_rule2_violation() {
        let err = derive_shape(ManifestKind::External, false, false).unwrap_err();
        assert_eq!(err, ManifestError::ExternalRequiresTool);
    }

    #[test]
    fn shape_external_with_methods_is_rejected() {
        // §1a.2e: `external + methods` is invalid (no describe).
        let err = derive_shape(ManifestKind::External, true, true).unwrap_err();
        assert_eq!(err, ManifestError::ExternalCannotDeclareMethods);
    }

    #[test]
    fn shape_external_no_tool_no_methods_is_rejected() {
        let err = derive_shape(ManifestKind::External, false, true).unwrap_err();
        // Either rule could fire first; rule 2 (external requires tool)
        // is the more specific and friendlier message — assert it.
        assert_eq!(err, ManifestError::ExternalRequiresTool);
    }

    // --- parse: happy paths for each shape ---------------------------------

    #[test]
    fn parses_tool_binary_manifest() {
        let json = r#"{
            "kind": "binary",
            "tool": {"name": "hello", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {"sha256": "4f9c3a4f9c3a4f9c3a4f9c3a4f9c3a4f9c3a4f9c3a4f9c3a4f9c3a4f9c3a4f9c", "size": 1234567}
            }
        }"#;
        let m = parse(json).unwrap();
        assert_eq!(m.kind, ManifestKind::Binary);
        assert_eq!(m.shape, ComponentShape::ToolBinary);
        assert_eq!(m.tool.as_ref().unwrap().name, "hello");
        assert!(m.methods.is_empty());
        assert_eq!(m.platforms.len(), 1);
    }

    #[test]
    fn parses_hybrid_component_manifest() {
        let json = r#"{
            "kind": "binary",
            "tool": {"name": "greet", "argv_passthrough": true, "description": "Friendly greeting"},
            "methods": ["greet"],
            "platforms": {
                "linux_amd64": {"sha256": "7e21b87e21b87e21b87e21b87e21b87e21b87e21b87e21b87e21b87e21b87e21"}
            }
        }"#;
        let m = parse(json).unwrap();
        assert_eq!(m.shape, ComponentShape::HybridComponent);
        assert_eq!(m.methods, vec!["greet".to_string()]);
        assert_eq!(
            m.tool.as_ref().unwrap().description.as_deref(),
            Some("Friendly greeting")
        );
    }

    #[test]
    fn parses_pure_component_manifest() {
        let json = r#"{
            "kind": "binary",
            "methods": ["status", "diff"],
            "platforms": {
                "linux_amd64": {"sha256": "abababababababababababababababababababababababababababababababab"}
            }
        }"#;
        let m = parse(json).unwrap();
        assert_eq!(m.shape, ComponentShape::Component);
        assert!(m.tool.is_none());
        assert_eq!(m.methods.len(), 2);
    }

    #[test]
    fn parses_tool_external_manifest() {
        let json = r#"{
            "kind": "external",
            "tool": {"name": "rg", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {
                    "sha256": "ad3a44e3d8b8a9d39c1f7b4d1a9b9e3a5e7c2f6c8b4f3a1d2e9c8b7a6e5d4c3b",
                    "url": "https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz",
                    "archive": "tar.gz",
                    "binary_in_archive": "ripgrep-14.1.1-x86_64-unknown-linux-musl/rg",
                    "archive_sha256": "4cf9f2741e6c465ffdb7c26f38056a59e2a2544b51f7cc128ef09337b3995f5f"
                }
            }
        }"#;
        let m = parse(json).unwrap();
        assert_eq!(m.kind, ManifestKind::External);
        assert_eq!(m.shape, ComponentShape::ToolExternal);
        let p = m.platforms.values().next().unwrap();
        assert!(matches!(p.archive, Archive::TarGz));
        assert_eq!(p.binary_in_archive.as_deref(), Some("ripgrep-14.1.1-x86_64-unknown-linux-musl/rg"));
        assert!(p.url.as_deref().unwrap().starts_with("https://"));
    }

    #[test]
    fn parses_external_bare_executable() {
        // archive: "none", no binary_in_archive.
        let json = r#"{
            "kind": "external",
            "tool": {"name": "jq", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {
                    "sha256": "5942c9b0934e510ee61eb3e30273f1b3fe2590df93933a93d7c58b81d19c8ff5",
                    "url": "https://github.com/jqlang/jq/releases/download/jq-1.7.1/jq-linux-amd64",
                    "archive": "none"
                }
            }
        }"#;
        let m = parse(json).unwrap();
        assert_eq!(m.shape, ComponentShape::ToolExternal);
        let p = m.platforms.values().next().unwrap();
        assert!(matches!(p.archive, Archive::None));
        assert!(p.binary_in_archive.is_none());
    }

    // --- parse: rule violations --------------------------------------------

    #[test]
    fn rejects_invalid_json() {
        let err = parse("{not json").unwrap_err();
        assert!(matches!(err, ManifestError::InvalidJson(_)));
    }

    #[test]
    fn rejects_missing_kind() {
        let json = r#"{"platforms": {}}"#;
        let err = parse(json).unwrap_err();
        assert_eq!(err, ManifestError::MissingKind);
    }

    #[test]
    fn rejects_unknown_kind() {
        let json = r#"{"kind": "magic", "platforms": {}}"#;
        let err = parse(json).unwrap_err();
        assert_eq!(err, ManifestError::UnknownKind("magic".to_string()));
    }

    #[test]
    fn rejects_external_without_tool() {
        let json = r#"{
            "kind": "external",
            "platforms": {
                "linux_amd64": {
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "url": "https://example.com/x",
                    "archive": "none"
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        assert_eq!(err, ManifestError::ExternalRequiresTool);
    }

    #[test]
    fn rejects_external_with_methods() {
        let json = r#"{
            "kind": "external",
            "tool": {"name": "x", "argv_passthrough": true},
            "methods": ["foo"],
            "platforms": {
                "linux_amd64": {
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "url": "https://example.com/x",
                    "archive": "none"
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        assert_eq!(err, ManifestError::ExternalCannotDeclareMethods);
    }

    #[test]
    fn rejects_binary_without_methods_or_tool() {
        let json = r#"{
            "kind": "binary",
            "platforms": {
                "linux_amd64": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
            }
        }"#;
        let err = parse(json).unwrap_err();
        assert_eq!(err, ManifestError::BinaryRequiresMethodsOrTool);
    }

    #[test]
    fn rejects_invalid_tool_name() {
        let json = r#"{
            "kind": "binary",
            "tool": {"name": "1bad", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
            }
        }"#;
        let err = parse(json).unwrap_err();
        let is_invalid = matches!(err, ManifestError::InvalidToolName(_));
        assert!(is_invalid, "got {err:?}");
    }

    #[test]
    fn rejects_http_url() {
        // §1a.2 rule 4: external URLs must be https://.
        let json = r#"{
            "kind": "external",
            "tool": {"name": "x", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "url": "http://example.com/x",
                    "archive": "none"
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        let is_url = matches!(err, ManifestError::InvalidUrl { .. });
        assert!(is_url, "got {err:?}");
    }

    #[test]
    fn rejects_file_url() {
        let json = r#"{
            "kind": "external",
            "tool": {"name": "x", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "url": "file:///etc/passwd",
                    "archive": "none"
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        let is_url = matches!(err, ManifestError::InvalidUrl { .. });
        assert!(is_url, "got {err:?}");
    }

    #[test]
    fn rejects_binary_with_url_field() {
        // §1a.2 rule 4: for `kind: binary`, url/archive/binary_in_archive
        // must all be absent.
        let json = r#"{
            "kind": "binary",
            "tool": {"name": "x", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "url": "https://example.com/x"
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        assert_eq!(err, ManifestError::BinaryKindForbidsField("url"));
    }

    #[test]
    fn rejects_external_without_url() {
        let json = r#"{
            "kind": "external",
            "tool": {"name": "x", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "archive": "none"
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        assert_eq!(err, ManifestError::ExternalKindRequires("url"));
    }

    #[test]
    fn rejects_archive_without_binary_in_archive() {
        let json = r#"{
            "kind": "external",
            "tool": {"name": "x", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "url": "https://example.com/x.tar.gz",
                    "archive": "tar.gz"
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        assert_eq!(err, ManifestError::ArchiveRequiresBinaryInArchive);
    }

    #[test]
    fn rejects_unsupported_os() {
        let json = r#"{
            "kind": "binary",
            "tool": {"name": "x", "argv_passthrough": true},
            "platforms": {
                "freebsd_amd64": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
            }
        }"#;
        let err = parse(json).unwrap_err();
        let is_os = matches!(err, ManifestError::UnsupportedOs(_));
        assert!(is_os, "got {err:?}");
    }

    #[test]
    fn rejects_unsupported_arch() {
        let json = r#"{
            "kind": "binary",
            "tool": {"name": "x", "argv_passthrough": true},
            "platforms": {
                "linux_riscv64": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
            }
        }"#;
        let err = parse(json).unwrap_err();
        let is_arch = matches!(err, ManifestError::UnsupportedArch(_));
        assert!(is_arch, "got {err:?}");
    }

    #[test]
    fn rejects_unsupported_archive() {
        let json = r#"{
            "kind": "external",
            "tool": {"name": "x", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "url": "https://example.com/x.rar",
                    "archive": "rar"
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        let is_arch = matches!(err, ManifestError::UnsupportedArchive(_));
        assert!(is_arch, "got {err:?}");
    }

    #[test]
    fn rejects_sha256_not_64_hex_chars() {
        let json = r#"{
            "kind": "binary",
            "tool": {"name": "x", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {"sha256": "tooshort"}
            }
        }"#;
        let err = parse(json).unwrap_err();
        let is_sha = matches!(err, ManifestError::InvalidSha256(_));
        assert!(is_sha, "got {err:?}");
    }

    #[test]
    fn rejects_binary_in_archive_with_dotdot() {
        let json = r#"{
            "kind": "external",
            "tool": {"name": "x", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "url": "https://example.com/x.tar.gz",
                    "archive": "tar.gz",
                    "binary_in_archive": "../etc/passwd"
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        let is_path = matches!(err, ManifestError::InvalidBinaryInArchive(_));
        assert!(is_path, "got {err:?}");
    }

    #[test]
    fn rejects_binary_in_archive_absolute_path() {
        let json = r#"{
            "kind": "external",
            "tool": {"name": "x", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {
                    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "url": "https://example.com/x.tar.gz",
                    "archive": "tar.gz",
                    "binary_in_archive": "/etc/passwd"
                }
            }
        }"#;
        let err = parse(json).unwrap_err();
        let is_path = matches!(err, ManifestError::InvalidBinaryInArchive(_));
        assert!(is_path, "got {err:?}");
    }

    // --- argv_passthrough = false is reserved -------------------------------

    #[test]
    fn rejects_argv_passthrough_false() {
        // §1a.1: false is reserved.
        let json = r#"{
            "kind": "binary",
            "tool": {"name": "x", "argv_passthrough": false},
            "platforms": {
                "linux_amd64": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
            }
        }"#;
        let err = parse(json).unwrap_err();
        assert_eq!(err, ManifestError::InvalidArgvPassthrough);
    }
}
