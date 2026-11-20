use std::fmt::Write as _;

use indicatif::HumanBytes;
use tabled::{Table, Tabled, settings::Style};

use crate::cli::output::OutputFormat;
use crate::services::component_binary::parent_invocation_id;

pub fn print(format: OutputFormat, result: &serde_json::Value) -> anyhow::Result<()> {
    if result.is_null() {
        return Ok(());
    }
    let effective = effective_format(format, parent_invocation_id().is_some());
    print!("{}", render_result(effective, result)?);
    Ok(())
}

fn effective_format(requested: OutputFormat, nested: bool) -> OutputFormat {
    if nested {
        OutputFormat::Json
    } else {
        requested
    }
}

fn render_result(format: OutputFormat, result: &serde_json::Value) -> anyhow::Result<String> {
    if matches!(format, OutputFormat::Json) {
        return Ok(format!("{}\n", serde_json::to_string_pretty(result)?));
    }

    if let Some(summary) = BuildSummaryView::from_value(result) {
        return Ok(summary.render(format));
    }

    Ok(format!("{}\n", serde_json::to_string_pretty(result)?))
}

struct BuildSummaryView {
    name: String,
    version: String,
    artifacts: Vec<Artifact>,
}

struct Artifact {
    os: String,
    arch: String,
    path: String,
    size: u64,
}

#[derive(Tabled)]
struct ArtifactRow {
    #[tabled(rename = "PLATFORM")]
    platform: String,
    #[tabled(rename = "SIZE")]
    size: String,
    #[tabled(rename = "ARTIFACT")]
    artifact: String,
}

impl BuildSummaryView {
    fn from_value(v: &serde_json::Value) -> Option<Self> {
        let obj = v.as_object()?;
        let raw = obj.get("artifacts")?.as_array()?;

        let mut artifacts = Vec::with_capacity(raw.len());
        for a in raw {
            let a = a.as_object()?;
            artifacts.push(Artifact {
                os: a.get("os")?.as_str()?.to_string(),
                arch: a.get("arch")?.as_str()?.to_string(),
                path: a.get("path")?.as_str()?.to_string(),
                size: a.get("size")?.as_u64()?,
            });
        }

        let name = obj.get("name")?.as_str()?.to_string();
        let version = obj
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Some(Self {
            name,
            version,
            artifacts,
        })
    }

    fn render(&self, format: OutputFormat) -> String {
        match format {
            OutputFormat::Pretty => self.render_pretty(),
            OutputFormat::Text => self.render_text(),
            OutputFormat::Name => self.render_name(),
            OutputFormat::Json => unreachable!("json is emitted verbatim, not reinterpreted"),
        }
    }

    fn render_pretty(&self) -> String {
        let mut out = String::new();

        let heading = if self.version.is_empty() {
            self.name.clone()
        } else {
            format!("{} v{}", self.name, self.version)
        };
        let n = self.artifacts.len();
        let noun = if n == 1 { "artifact" } else { "artifacts" };
        let _ = writeln!(out, "{heading}  ({n} {noun})");

        if !self.artifacts.is_empty() {
            out.push('\n');
            let rows: Vec<ArtifactRow> = self
                .artifacts
                .iter()
                .map(|a| ArtifactRow {
                    platform: format!("{}/{}", a.os, a.arch),
                    size: HumanBytes(a.size).to_string(),
                    artifact: a.path.clone(),
                })
                .collect();
            let mut table = Table::new(rows);
            table.with(Style::rounded());
            let _ = writeln!(out, "{table}");
        }

        out
    }

    fn render_text(&self) -> String {
        let mut out = String::new();
        for a in &self.artifacts {
            let _ = writeln!(out, "{}/{}\t{}\t{}", a.os, a.arch, a.size, a.path);
        }
        out
    }

    fn render_name(&self) -> String {
        let mut out = String::new();
        for a in &self.artifacts {
            out.push_str(&a.path);
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn build_summary() -> serde_json::Value {
        json!({
            "name": "build-rust-example",
            "version": "0.1.0",
            "artifacts": [
                { "os": "linux", "arch": "amd64", "path": "/out/linux/amd64/x", "sha256": "aa", "size": 12_300_000u64 },
                { "os": "macos", "arch": "arm64", "path": "/out/macos/arm64/x", "sha256": "bb", "size": 11_200_000u64 }
            ]
        })
    }

    #[test]
    fn nested_forces_json_regardless_of_requested_format() {
        assert_eq!(
            effective_format(OutputFormat::Pretty, true),
            OutputFormat::Json
        );
        assert_eq!(
            effective_format(OutputFormat::Text, true),
            OutputFormat::Json
        );
        assert_eq!(
            effective_format(OutputFormat::Pretty, false),
            OutputFormat::Pretty
        );
    }

    #[test]
    fn json_format_emits_result_verbatim() {
        let out = render_result(OutputFormat::Json, &build_summary()).unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(back, build_summary());
    }

    #[test]
    fn pretty_reinterprets_build_summary() {
        let out = render_result(OutputFormat::Pretty, &build_summary()).unwrap();
        assert!(out.contains("build-rust-example v0.1.0"));
        assert!(out.contains("(2 artifacts)"));
        assert!(out.contains("linux/amd64"));
        assert!(out.contains("macos/arm64"));
        assert!(out.contains("MiB"));
        assert!(!out.contains("12300000"));
    }

    #[test]
    fn text_uses_raw_bytes_and_tabs() {
        let out = render_result(OutputFormat::Text, &build_summary()).unwrap();
        assert!(out.contains("linux/amd64\t12300000\t/out/linux/amd64/x"));
    }

    #[test]
    fn name_emits_artifact_paths() {
        let out = render_result(OutputFormat::Name, &build_summary()).unwrap();
        assert_eq!(out, "/out/linux/amd64/x\n/out/macos/arm64/x\n");
    }

    #[test]
    fn unknown_shape_falls_back_to_pretty_json() {
        let v = json!({ "hello": "world" });
        let out = render_result(OutputFormat::Pretty, &v).unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn foreign_artifacts_payload_is_not_mistaken_for_build_summary() {
        let v = json!({ "artifacts": [{ "url": "https://x" }] });
        assert!(BuildSummaryView::from_value(&v).is_none());
    }
}
