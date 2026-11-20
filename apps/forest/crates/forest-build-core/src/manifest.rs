use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

use crate::{FOREST_VERSION_ENV, Toolchain};

fn version_override() -> Option<String> {
    let raw = std::env::var(FOREST_VERSION_ENV).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[derive(Debug)]
pub struct BuildRequest {
    pub name: String,
    pub version: String,
    pub source: PathBuf,
    pub out_base: PathBuf,
    pub architectures: HashMap<String, HashMap<String, serde_json::Value>>,
}

impl Toolchain {
    pub fn upload_type(self) -> &'static str {
        match self {
            Toolchain::Rust => "rust",
            Toolchain::Golang => "go",
            Toolchain::Docker => "docker",
        }
    }
}

/// Fails if the declared `upload.type` does not match `toolchain`.
pub async fn read_build_request(
    toolchain: Toolchain,
    work_dir: &Path,
) -> anyhow::Result<BuildRequest> {
    let doc = export_cue(work_dir).await?;

    let component = doc
        .pointer("/forest/component")
        .context("forest.component section is required to build")?;

    let name = component
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| doc.pointer("/project/name").and_then(|v| v.as_str()))
        .context("forest.component.name (or project.name) is required")?
        .to_string();

    let cue_version = component
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.1.0");
    let version = version_override().unwrap_or_else(|| cue_version.to_string());
    if version != cue_version {
        tracing::info!("component version overridden by {FOREST_VERSION_ENV}: {version}");
    }

    let upload = component
        .get("upload")
        .context("forest.component.upload section is required for building")?;

    let upload_type = upload.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if upload_type != toolchain.upload_type() {
        bail!(
            "this is the {} build component, but the project declares \
             upload.type=\"{}\". Depend on the matching build component instead.",
            toolchain.upload_type(),
            upload_type,
        );
    }

    let source_rel = upload.get("source").and_then(|v| v.as_str()).unwrap_or(".");
    let source = work_dir.join(source_rel);

    let architectures = upload
        .get("architectures")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(os, arches)| {
                    let arches = arches.as_object()?;
                    let inner = arches
                        .iter()
                        .map(|(arch, v)| (arch.clone(), v.clone()))
                        .collect::<HashMap<_, _>>();
                    Some((os.clone(), inner))
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    if architectures.is_empty() {
        bail!("forest.component.upload.architectures must declare at least one platform");
    }

    Ok(BuildRequest {
        name,
        version,
        source,
        out_base: work_dir.join(".forest/component/output"),
        architectures,
    })
}

async fn export_cue(work_dir: &Path) -> anyhow::Result<serde_json::Value> {
    let mut cue_files = Vec::new();
    for entry in
        std::fs::read_dir(work_dir).with_context(|| format!("reading {}", work_dir.display()))?
    {
        let entry = entry?;
        if entry.path().extension().and_then(|e| e.to_str()) == Some("cue") {
            cue_files.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    if cue_files.is_empty() {
        bail!(
            "no .cue files found in {} — is this a forest component directory?",
            work_dir.display()
        );
    }
    cue_files.sort();

    let mut cmd = tokio::process::Command::new("cue");
    cmd.current_dir(work_dir).arg("export");
    for f in &cue_files {
        cmd.arg(f);
    }
    cmd.args(["--out", "json"]);

    let output = cmd
        .output()
        .await
        .context("failed to run `cue export` (is `cue` on PATH?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("failed to evaluate CUE manifest:\n{stderr}");
    }

    let doc: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("cue export produced invalid JSON")?;
    Ok(doc)
}
