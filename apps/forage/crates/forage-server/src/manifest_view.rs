use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ManifestView {
    #[serde(default)]
    pub kind: String,

    #[serde(default)]
    pub platforms: BTreeMap<String, PlatformInfo>,

    #[serde(default)]
    pub methods: Vec<String>,

    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub archive: Option<String>,
    #[serde(default)]
    pub binary_in_archive: Option<String>,
    #[serde(default)]
    pub archive_sha256: Option<String>,

    #[serde(default)]
    pub include: IncludeView,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct IncludeView {
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PlatformInfo {
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

impl ManifestView {
    pub fn parse(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }

    pub fn short_sha(sha: &str) -> String {
        let trimmed = sha.strip_prefix("sha256:").unwrap_or(sha);
        if trimmed.len() <= 12 {
            return trimmed.to_string();
        }
        format!("{}…{}", &trimmed[..6], &trimmed[trimmed.len() - 5..])
    }

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
        assert!(m.methods.is_empty());
    }

    #[test]
    fn parse_include_env() {
        let json = r#"{
            "kind": "binary",
            "tool": {"name": "fungus", "argv_passthrough": true},
            "include": {"env": {"FUNGUS_SERVER": "https://fungus.understory.sh"}},
            "platforms": {"darwin_arm64": {"sha256": "abc", "size": 1}}
        }"#;
        let m = ManifestView::parse(json).unwrap();
        assert_eq!(
            m.include.env.get("FUNGUS_SERVER").map(String::as_str),
            Some("https://fungus.understory.sh")
        );
    }

    #[test]
    fn manifest_without_include_has_empty_env() {
        let json = r#"{"kind": "binary", "platforms": {}}"#;
        let m = ManifestView::parse(json).unwrap();
        assert!(m.include.env.is_empty());
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
    fn short_sha_truncates_long_hex() {
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
