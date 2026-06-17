use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use forest_build_core::Toolchain;
use serde::{Deserialize, Serialize};

use crate::state::State;

/// Build the component binary for all configured platforms.
///
/// Reads forest.cue and spec.cue to determine the component name,
/// version, and target architectures. Compiles the binary (Rust, Go,
/// or Docker) via `forest-build-core`, stores it in the content-addressable
/// cache, and caches the component descriptor for fast command discovery.
///
/// Output: ~/.cache/forest/components/bin/{hash}
/// Metadata: ~/.cache/forest/components/<org>/<name>/<version>/.forest/component/meta.json
#[derive(clap::Parser)]
pub struct BuildCommand {}

impl BuildCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        // Try forest.component.cue first (new SDK pattern), fall back to spec.cue (legacy)
        let cue_files = if std::path::Path::new("forest.component.cue").exists() {
            vec!["./forest.cue", "./forest.component.cue"]
        } else {
            vec!["./forest.cue", "./spec.cue"]
        };
        let output = crate::tools::cue::output(|| {
            let mut cmd = tokio::process::Command::new("cue");
            cmd.arg("export");
            for f in &cue_files {
                cmd.arg(f);
            }
            cmd.args(["--out", "json"]);
            if let Ok(registry) = std::env::var("CUE_REGISTRY") {
                cmd.env("CUE_REGISTRY", registry);
            }
            cmd
        })
        .await?;
        let stdout = String::from_utf8(output.stdout)?;
        let stderr = String::from_utf8(output.stderr)?;

        if !output.status.success() {
            if stderr.contains("no such file or directory") || stderr.contains("does not exist") {
                anyhow::bail!(
                    "no forest.cue or spec.cue found in current directory.\n\
                     Are you in a component directory? Run `forest components init <name>` to create one."
                );
            }
            anyhow::bail!("failed to evaluate CUE spec:\n{stderr}");
        }

        let doc: Document = serde_json::from_str(stdout.trim())?;

        let Some(component) = &doc.forest.as_ref().and_then(|f| f.component.as_ref()) else {
            anyhow::bail!("cannot build when no forest.component section is set");
        };

        let Some(upload) = &component.upload else {
            anyhow::bail!("forest.component.upload section is required for building");
        };

        let organisation = doc
            .project
            .as_ref()
            .and_then(|p| p.organisation.as_deref())
            .unwrap_or("forest");

        // Prebuilt components carry their binaries on disk — nothing to
        // compile. `forest publish` reads `upload.prebuilt` directly.
        if matches!(upload.source_type, SourceType::Prebuilt) {
            tracing::info!(
                "component '{}' uses upload.type=prebuilt — skipping build",
                component.name,
            );
            return Ok(());
        }

        // Deno/TypeScript components: auto-run codegen if stale, then generate meta.json
        if matches!(upload.source_type, SourceType::Deno | SourceType::Typescript) {
            // Auto-run codegen if forest.component.cue is newer than forestgen output
            if let Some(codegen) = &component.codegen {
                let spec_path = std::env::current_dir()?.join("forest.component.cue");
                let gen_path = std::path::PathBuf::from(&codegen.output).join("forestgen.ts");
                let needs_codegen = match (spec_path.metadata(), gen_path.metadata()) {
                    (Ok(spec_meta), Ok(gen_meta)) => {
                        spec_meta.modified().ok() > gen_meta.modified().ok()
                    }
                    (Ok(_), Err(_)) => true, // forestgen.ts doesn't exist
                    _ => false,
                };
                if needs_codegen {
                    tracing::info!(
                        "forest.component.cue is newer than forestgen.ts — regenerating codegen"
                    );
                    let generate = super::generate::GenerateCommand {
                        output: Some(std::path::PathBuf::from(&codegen.output)),
                        language: None,
                    };
                    generate.execute(state).await?;
                }
            }
            let entrypoint = upload.source.join("main.ts");
            tracing::info!(
                "deno component '{}' — generating meta.json",
                component.name,
            );

            // Run _meta/describe to get the descriptor
            let descriptor = crate::services::component_deno::describe_deno_component(
                &std::env::current_dir()?,
                &entrypoint.to_string_lossy(),
            )
            .await
            .ok();

            let meta_dir = crate::services::component_binary::component_meta_dir(
                organisation,
                &component.name,
                &component.version,
            )
            .context("failed to resolve component cache directory")?;
            std::fs::create_dir_all(&meta_dir)?;
            let mut meta = serde_json::json!({
                "organisation": organisation,
                "name": component.name,
                "version": component.version,
                "kind": "deno",
                "entrypoint": entrypoint.to_string_lossy(),
            });
            if let Some(desc) = descriptor {
                meta["descriptor"] = serde_json::to_value(&desc)?;
            }
            std::fs::write(
                meta_dir.join("meta.json"),
                serde_json::to_string_pretty(&meta)?,
            )?;

            tracing::info!(
                "meta.json generated for deno component at {}",
                meta_dir.display()
            );
            return Ok(());
        }

        let architectures = upload
            .architectures
            .as_ref()
            .context("architectures section is required for building")?;

        let toolchain = upload.source_type.toolchain().context(
            "upload.type does not map to a compiled toolchain",
        )?;

        let targets = forest_build_core::resolve_targets(architectures, toolchain)?;

        if targets.is_empty() {
            anyhow::bail!("no build targets resolved from architectures");
        }

        let out_base = output_base_dir()?;

        tracing::info!(
            "building {} target(s) for component '{}'",
            targets.len(),
            component.name,
        );

        for target in &targets {
            tracing::info!("building {}/{} ...", target.os, target.arch);
            forest_build_core::build_target(
                toolchain,
                &component.name,
                &component.version,
                &upload.source,
                &out_base,
                target,
            )
            .await?;
        }

        forest_build_core::generate_checksums(&component.name, &targets, &out_base)?;

        // Store built binaries in content-addressable cache and write meta.json
        let mut platforms = serde_json::Map::new();

        for target in &targets {
            let src = forest_build_core::output_dir(&out_base, &target.os, &target.arch)?
                .join(forest_build_core::output_filename(&component.name, target));
            let binary_content = std::fs::read(&src)
                .with_context(|| format!("read built binary {}", src.display()))?;

            let (sha256, cache_path) =
                crate::services::component_binary::store_binary_in_cache(&binary_content)?;

            let platform_key = format!("{}_{}", target.os, target.arch);
            platforms.insert(
                platform_key,
                serde_json::json!({
                    "sha256": sha256,
                    "size": binary_content.len(),
                }),
            );

            tracing::info!(
                "cached binary at {} (sha256={})",
                cache_path.display(),
                &sha256[..12]
            );
        }

        // Run _meta/describe on the current platform binary to cache the descriptor
        let (current_os, current_arch) = crate::services::component_binary::current_platform();
        let current_platform_key = format!("{current_os}_{current_arch}");
        let descriptor = if let Some(platform_info) = platforms.get(&current_platform_key) {
            if let Some(sha256) = platform_info.get("sha256").and_then(|v| v.as_str()) {
                if let Some(binary_path) =
                    crate::services::component_binary::resolve_binary_from_hash(sha256)
                {
                    match crate::services::component_binary::describe_component(&binary_path).await
                    {
                        Ok(desc) => {
                            tracing::info!("cached descriptor: {} methods", desc.methods.len());
                            Some(serde_json::to_value(&desc)?)
                        }
                        Err(e) => {
                            tracing::warn!("failed to describe component: {e}");
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Write meta.json with binary hashes + cached descriptor
        let meta_dir = crate::services::component_binary::component_meta_dir(
            organisation,
            &component.name,
            &component.version,
        )
        .context("failed to resolve component cache directory")?;
        std::fs::create_dir_all(&meta_dir)?;
        let mut meta = serde_json::json!({
            "organisation": organisation,
            "name": component.name,
            "version": component.version,
            "platforms": platforms,
        });
        if let Some(desc) = descriptor {
            meta["descriptor"] = desc;
        }
        std::fs::write(
            meta_dir.join("meta.json"),
            serde_json::to_string_pretty(&meta)?,
        )?;

        tracing::info!("all targets built successfully");
        Ok(())
    }
}

fn output_base_dir() -> anyhow::Result<PathBuf> {
    let cur_dir = std::env::current_dir()?;
    Ok(cur_dir.join(".forest/component/output"))
}

// --- Models ---

#[derive(Debug, Serialize, Deserialize)]
pub struct Document {
    project: Option<ProjectMeta>,
    forest: Option<Forest>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub name: Option<String>,
    pub organisation: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Forest {
    pub component: Option<Component>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Component {
    pub name: String,
    pub version: String,
    pub codegen: Option<Codegen>,
    pub upload: Option<Upload>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Codegen {
    #[serde(rename = "type")]
    pub source_type: SourceType,
    pub output: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Upload {
    #[serde(rename = "type")]
    pub source_type: SourceType,
    pub source: PathBuf,
    pub registry: String,
    pub architectures: Option<HashMap<String, HashMap<String, serde_json::Value>>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum SourceType {
    #[serde(rename = "rust")]
    Rust,
    #[serde(rename = "go")]
    Golang,
    #[serde(rename = "docker")]
    Docker,
    #[serde(rename = "deno")]
    Deno,
    #[serde(rename = "typescript")]
    Typescript,
    /// Author-supplied binaries listed per-platform under `upload.prebuilt`.
    /// `forest build` is a no-op; `forest publish` handles the upload.
    #[serde(rename = "prebuilt")]
    Prebuilt,
}

impl SourceType {
    /// Map to the `forest-build-core` toolchain, or `None` for source types
    /// that don't compile (deno/typescript/prebuilt).
    fn toolchain(&self) -> Option<Toolchain> {
        match self {
            SourceType::Rust => Some(Toolchain::Rust),
            SourceType::Golang => Some(Toolchain::Golang),
            SourceType::Docker => Some(Toolchain::Docker),
            SourceType::Deno | SourceType::Typescript | SourceType::Prebuilt => None,
        }
    }
}
