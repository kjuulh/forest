use std::path::PathBuf;

use tokio::io::AsyncWriteExt;

use crate::state::State;

/// Generate SDK code from the CUE component spec.
///
/// Reads forest.component.cue (and spec.cue if present), converts to OpenAPI
/// via `cue def`, then generates typed code into the output directory.
///
/// The language is auto-detected from forest.cue codegen.type field,
/// or can be overridden with --language.
///
/// Example: forest generate --output ./src/
#[derive(clap::Parser)]
pub struct GenerateCommand {
    /// Output directory for the generated code
    #[arg(long)]
    output: PathBuf,

    /// Language to generate (auto-detected from forest.cue if not specified)
    #[arg(long)]
    language: Option<String>,
}

impl GenerateCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        // Detect language from forest.cue or CLI flag
        let language = match &self.language {
            Some(lang) => lang.clone(),
            None => detect_codegen_language().await.unwrap_or_else(|| "rust".to_string()),
        };

        let codegen_language = match language.as_str() {
            "rust" => forest_sdk_codegen::CodegenLanguage::Rust,
            "typescript" | "deno" | "ts" => forest_sdk_codegen::CodegenLanguage::TypeScript,
            other => anyhow::bail!("unsupported codegen language: {other} (supported: rust, typescript)"),
        };

        // Build cue def args — include all .cue files that exist
        let mut cue_files = vec!["./forest.component.cue".to_string()];
        if tokio::fs::metadata("./spec.cue").await.is_ok() {
            cue_files.push("./spec.cue".to_string());
        }

        let mut cmd = tokio::process::Command::new("cue");
        if let Ok(registry) = std::env::var("CUE_REGISTRY") {
            cmd.env("CUE_REGISTRY", registry);
        }
        cmd.arg("def");
        for f in &cue_files {
            cmd.arg(f);
        }
        cmd.args(["--out", "openapi"]);

        let output = cmd.output().await?;
        let stdout = String::from_utf8(output.stdout)?;
        let stderr = String::from_utf8(output.stderr)?;
        if !output.status.success() {
            anyhow::bail!("failed to generate spec from cue: {stdout}, {stderr}");
        }

        let codegen = forest_sdk_codegen::Codegen {
            options: forest_sdk_codegen::CodegenOptions {
                destination: self.output.display().to_string(),
                language: codegen_language,
            },
        };

        let generated_code = codegen.generate(stdout.trim())?;

        tokio::fs::create_dir_all(&self.output).await?;

        let filename = match language.as_str() {
            "typescript" | "deno" | "ts" => "forestgen.ts",
            _ => "forestgen.rs",
        };

        let mut file = tokio::fs::File::create(self.output.join(filename)).await?;
        file.write_all(generated_code.as_bytes()).await?;
        file.flush().await?;

        tracing::info!("generated {} at {}", filename, self.output.display());
        Ok(())
    }
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
