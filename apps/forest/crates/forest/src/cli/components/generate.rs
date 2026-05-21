use std::path::PathBuf;

use anyhow::Context;
use tokio::io::AsyncWriteExt;

use crate::state::State;

/// Generate typed TypeScript (or Rust) SDK code and dependency clients from the CUE component spec.
///
/// Reads forest.component.cue (and spec.cue if present), converts to OpenAPI
/// via `cue def`, then generates typed code into the output directory.
/// For TypeScript projects, also generates typed dependency clients for any
/// local component dependencies that have a forest.component.cue file.
///
/// Output files:
///   <output>/forestgen.ts   — component types and handler scaffolding (TypeScript)
///   <output>/forestgen.rs   — component types and handler scaffolding (Rust)
///   <output>/deps/<name>.ts — typed client for each local component dependency
///
/// Example: forest generate --output ./src/
#[derive(clap::Parser)]
pub struct GenerateCommand {
    /// Output directory for the generated code (defaults to codegen.output from forest.cue)
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Language to generate (auto-detected from forest.cue if not specified)
    #[arg(long)]
    pub language: Option<String>,
}

impl GenerateCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        let language = match &self.language {
            Some(lang) => lang.clone(),
            None => detect_codegen_language().await.unwrap_or_else(|| "rust".to_string()),
        };

        let output = match &self.output {
            Some(o) => o.clone(),
            None => detect_codegen_output()
                .await
                .ok_or_else(|| anyhow::anyhow!(
                    "no --output specified and no codegen.output found in forest.cue"
                ))?,
        };

        let codegen_language = match language.as_str() {
            "rust" => forest_sdk_codegen::CodegenLanguage::Rust,
            "typescript" | "deno" | "ts" => forest_sdk_codegen::CodegenLanguage::TypeScript,
            other => anyhow::bail!("unsupported codegen language: {other}"),
        };

        let codegen = forest_sdk_codegen::Codegen {
            options: forest_sdk_codegen::CodegenOptions {
                destination: output.display().to_string(),
                language: codegen_language,
            },
        };

        // Generate own component types
        let openapi_json = run_cue_def_openapi(&["./forest.component.cue"]).await?;
        let generated_code = codegen.generate(openapi_json.trim())?;

        tokio::fs::create_dir_all(&output).await?;

        let filename = match language.as_str() {
            "typescript" | "deno" | "ts" => "forestgen.ts",
            _ => "forestgen.rs",
        };

        let mut file = tokio::fs::File::create(output.join(filename)).await?;
        file.write_all(generated_code.as_bytes()).await?;
        file.flush().await?;
        tracing::info!("generated {} at {}", filename, output.display());

        // Generate dependency clients
        if matches!(language.as_str(), "typescript" | "deno" | "ts") {
            self.generate_dependency_clients(&output, &codegen).await?;
        }

        Ok(())
    }

    /// Discover local component dependencies and generate typed clients for them.
    async fn generate_dependency_clients(
        &self,
        output: &std::path::Path,
        codegen: &forest_sdk_codegen::Codegen,
    ) -> anyhow::Result<()> {
        let deps = discover_component_dependencies().await?;
        if deps.is_empty() {
            return Ok(());
        }

        let deps_dir = output.join("deps");
        tokio::fs::create_dir_all(&deps_dir).await?;

        for (component_id, component_path) in deps {
            let component_cue = component_path.join("forest.component.cue");
            if !component_cue.exists() {
                continue;
            }

            tracing::info!("generating dependency client for {}", component_id);

            let openapi_json = run_cue_def_openapi_in_dir(
                &[&component_cue.to_string_lossy()],
                &component_path,
            )
            .await
            .with_context(|| format!("cue def for dependency {component_id}"))?;

            let client_code = codegen
                .generate_client(openapi_json.trim(), &component_id)
                .with_context(|| format!("generate client for {component_id}"))?;

            let safe_name = component_id.replace('/', "_");
            let client_file = deps_dir.join(format!("{safe_name}.ts"));

            let mut file = tokio::fs::File::create(&client_file).await?;
            file.write_all(client_code.as_bytes()).await?;
            file.flush().await?;

            tracing::info!("generated dependency client: {}", client_file.display());
        }

        Ok(())
    }
}

/// Run `cue def --out openapi` on the given files in the current directory.
async fn run_cue_def_openapi(cue_files: &[&str]) -> anyhow::Result<String> {
    run_cue_def_openapi_in_dir(cue_files, &std::env::current_dir()?).await
}

/// Run `cue def --out openapi` on the given files in a specific directory.
async fn run_cue_def_openapi_in_dir(
    cue_files: &[&str],
    dir: &std::path::Path,
) -> anyhow::Result<String> {
    let output = crate::tools::cue::output(|| {
        let mut cmd = tokio::process::Command::new("cue");
        if let Ok(registry) = std::env::var("CUE_REGISTRY") {
            cmd.env("CUE_REGISTRY", registry);
        }
        cmd.arg("def");
        for f in cue_files {
            cmd.arg(f);
        }
        cmd.args(["--out", "openapi"]);
        cmd.current_dir(dir);
        cmd
    })
    .await?;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    if !output.status.success() {
        anyhow::bail!("cue def failed: {stdout}, {stderr}");
    }
    Ok(stdout)
}

/// Parse forest.cue to find local component dependencies.
/// Returns (component_id, local_path) pairs for dependencies that are local paths
/// and have a forest.component.cue file (i.e., they are components, not just CUE modules).
async fn discover_component_dependencies() -> anyhow::Result<Vec<(String, PathBuf)>> {
    // Read forest.cue to get dependencies
    let output = crate::tools::cue::output(|| {
        let mut cmd = tokio::process::Command::new("cue");
        if let Ok(registry) = std::env::var("CUE_REGISTRY") {
            cmd.env("CUE_REGISTRY", registry);
        }
        cmd.args(["export", "--out", "json", "forest.cue"]);
        cmd
    })
    .await?;
    if !output.status.success() {
        return Ok(vec![]);
    }

    let doc: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("parse forest.cue JSON")?;

    let Some(deps) = doc.get("dependencies").and_then(|d| d.as_object()) else {
        return Ok(vec![]);
    };

    let mut result = Vec::new();
    let cwd = std::env::current_dir()?;

    for (name, spec) in deps {
        // Only local path dependencies
        if let Some(path) = spec.get("path").and_then(|p| p.as_str()) {
            let dep_path = cwd.join(path);
            if dep_path.join("forest.component.cue").exists() {
                result.push((name.clone(), dep_path));
            }
        }
    }

    Ok(result)
}

/// Try to detect the codegen output directory from forest.cue's codegen.output field.
async fn detect_codegen_output() -> Option<PathBuf> {
    let output = tokio::process::Command::new("cue")
        .args(["export", "--out", "json", "forest.cue"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    doc.get("forest")
        .and_then(|f| f.get("component"))
        .and_then(|c| c.get("codegen"))
        .and_then(|c| c.get("output"))
        .and_then(|t| t.as_str())
        .map(PathBuf::from)
}

/// Try to detect the codegen language from forest.cue's codegen.type field.
async fn detect_codegen_language() -> Option<String> {
    let output = tokio::process::Command::new("cue")
        .args(["export", "--out", "json", "forest.cue"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    doc.get("forest")
        .and_then(|f| f.get("component"))
        .and_then(|c| c.get("codegen"))
        .and_then(|c| c.get("type"))
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}
