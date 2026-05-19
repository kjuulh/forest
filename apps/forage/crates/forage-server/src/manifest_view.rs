//! Parsed view of a component manifest, structured for UI rendering.
//!
//! We intentionally don't reuse forest's `forest-manifest` crate here — the UI
//! only needs to surface fields, not validate them. A permissive serde-driven
//! reader keeps forage independent of the upstream validator's strictness.
//!
//! Source of truth for the schema: TASKS/018-global-tools.md §1a.2 (kind,
//! tool, methods, platforms, external block).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Everything the detail page wants to render from a manifest JSON blob.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ManifestView {
    /// "binary" | "external" | "files" — taxonomy from §1a.2.
    #[serde(default)]
    pub kind: String,

    /// Per-platform binary info, keyed by `<os>_<arch>` (e.g. "linux_amd64").
    #[serde(default)]
    pub platforms: BTreeMap<String, PlatformInfo>,

    /// Methods this component exposes (HYBRID + COMPONENT shapes).
    #[serde(default)]
    pub methods: Vec<String>,

    /// External-tool fields. `None` for binary kinds.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub archive: Option<String>,
    #[serde(default)]
    pub binary_in_archive: Option<String>,
    #[serde(default)]
    pub archive_sha256: Option<String>,
}

/// Per-platform metadata extracted from the manifest's `platforms` map.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PlatformInfo {
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

impl ManifestView {
    /// Parse a manifest JSON string. Returns `None` if the input isn't valid
    /// JSON or doesn't deserialize — the detail page falls back to the raw-
    /// JSON viewer in that case.
    pub fn parse(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }

    /// `true` when no native UI section will render anything useful; the
    /// caller can decide to suppress the wrapper card entirely.
    pub fn is_empty(&self) -> bool {
        self.platforms.is_empty()
            && self.methods.is_empty()
            && self.url.is_none()
            && self.archive.is_none()
            && self.binary_in_archive.is_none()
    }

    /// Render a sha256 in a compact human form: `5df1c9…ec945` (first 6 +
    /// last 5 of the hex digest). Manifest displays use the full digest
    /// elsewhere (clipboard copy), this is for inline chips.
    pub fn short_sha(sha: &str) -> String {
        let trimmed = sha.strip_prefix("sha256:").unwrap_or(sha);
        if trimmed.len() <= 12 {
            return trimmed.to_string();
        }
        format!("{}…{}", &trimmed[..6], &trimmed[trimmed.len() - 5..])
    }

    /// Format a byte count in a human-readable unit. Matches the convention
    /// used elsewhere in forage (KB/MB/GB binary-ish, two-decimal precision
    /// until we hit MB+).
    pub fn human_size(bytes: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;
        if bytes < KB {
            format!("{} B", bytes)
        } else if bytes < MB {
            format!("{:.1} KB", bytes as f64 / KB as f64)
        } else if bytes < GB {
            format!("{:.1} MB", bytes as f64 / MB as f64)
        } else {
            format!("{:.2} GB", bytes as f64 / GB as f64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tool_binary_manifest() {
        let json = r#"{
            "kind": "binary",
            "tool": {"name": "forest-hello", "argv_passthrough": true},
            "platforms": {
                "linux_amd64": {"sha256": "abc123", "size": 438888},
                "darwin_arm64": {"sha256": "def456", "size": 512000}
            }
        }"#;
        let m = ManifestView::parse(json).unwrap();
        assert_eq!(m.kind, "binary");
        assert_eq!(m.platforms.len(), 2);
        assert_eq!(m.platforms.get("linux_amd64").unwrap().size, 438888);
        assert!(m.url.is_none());
        assert!(m.archive.is_none());
        // Methods absent → empty vec (forward-compat: this is a tool_binary
        // with no methods, which is exactly the shape spec).
        assert!(m.methods.is_empty());
    }

    #[test]
    fn parse_external_manifest() {
        let json = r#"{
            "kind": "external",
            "tool": {"name": "rg", "argv_passthrough": true},
            "url": "https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/ripgrep.tar.gz",
            "archive": "tar.gz",
            "binary_in_archive": "ripgrep-14.1.1/rg",
            "archive_sha256": "sha256:facebee",
            "platforms": {
                "linux_amd64": {"sha256": "deadbeef", "size": 0}
            }
        }"#;
        let m = ManifestView::parse(json).unwrap();
        assert_eq!(m.kind, "external");
        assert_eq!(m.url.as_deref(), Some("https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/ripgrep.tar.gz"));
        assert_eq!(m.archive.as_deref(), Some("tar.gz"));
        assert_eq!(m.binary_in_archive.as_deref(), Some("ripgrep-14.1.1/rg"));
        assert_eq!(m.archive_sha256.as_deref(), Some("sha256:facebee"));
    }

    #[test]
    fn parse_hybrid_with_methods() {
        let json = r#"{
            "kind": "binary",
            "tool": {"name": "greet"},
            "methods": ["greet", "status"]
        }"#;
        let m = ManifestView::parse(json).unwrap();
        assert_eq!(m.methods, vec!["greet".to_string(), "status".to_string()]);
    }

    #[test]
    fn parse_returns_none_for_garbage() {
        assert!(ManifestView::parse("not json").is_none());
        assert!(ManifestView::parse("").is_none());
    }

    #[test]
    fn is_empty_when_nothing_to_render() {
        let m = ManifestView::default();
        assert!(m.is_empty());

        let m = ManifestView::parse(r#"{"kind": "files"}"#).unwrap();
        assert!(m.is_empty(), "files manifest with no other fields renders nothing");
    }

    #[test]
    fn short_sha_truncates_long_hex() {
        // 64-char hex digest → 6-prefix + ellipsis + 5-suffix.
        let full = "5df1c90d18b8cba88100df635f1914f900ebdf17be6652a6ae17a5833ceec945";
        assert_eq!(ManifestView::short_sha(full), "5df1c9…ec945");
    }

    #[test]
    fn short_sha_strips_sha256_prefix() {
        let prefixed = "sha256:5df1c90d18b8cba88100df635f1914f900ebdf17be6652a6ae17a5833ceec945";
        assert_eq!(ManifestView::short_sha(prefixed), "5df1c9…ec945");
    }

    #[test]
    fn short_sha_leaves_short_input_intact() {
        // Short input doesn't get an ellipsis added — useful for tests + edge
        // cases like empty/truncated manifests.
        assert_eq!(ManifestView::short_sha("abc"), "abc");
        assert_eq!(ManifestView::short_sha("123456789012"), "123456789012");
    }

    #[test]
    fn human_size_picks_right_unit() {
        assert_eq!(ManifestView::human_size(0), "0 B");
        assert_eq!(ManifestView::human_size(999), "999 B");
        assert_eq!(ManifestView::human_size(2048), "2.0 KB");
        assert_eq!(ManifestView::human_size(1_572_864), "1.5 MB");
        assert_eq!(ManifestView::human_size(2_147_483_648), "2.00 GB");
    }
}
