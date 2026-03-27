//! Deno/TypeScript component invocation.
//!
//! Mirrors `component_binary.rs` but spawns `deno run` instead of a native binary.
//! The JSON stdin/stdout protocol is identical — Deno components speak the same
//! protocol as Rust binary components.

use std::path::Path;

use anyhow::Context;
use tokio::io::AsyncWriteExt;

use std::time::Duration;

const COMPONENT_TIMEOUT: Duration = Duration::from_secs(120);
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Check if Deno is available on the system.
pub async fn check_deno_available() -> anyhow::Result<()> {
    match tokio::process::Command::new("deno")
        .arg("--version")
        .output()
        .await
    {
        Ok(output) if output.status.success() => Ok(()),
        Ok(_) => anyhow::bail!(
            "deno is installed but returned an error. Check your Deno installation."
        ),
        Err(_) => anyhow::bail!(
            "deno is not installed. Install it from https://deno.land\n\
             Components written in TypeScript require the Deno runtime."
        ),
    }
}

/// Detect if a component directory contains a Deno component.
///
/// Checks for `meta.json` with `kind: "deno"`, or the presence of
/// a TypeScript entrypoint (`src/main.ts`).
pub fn is_deno_component(path: &Path) -> bool {
    // Check meta.json first (cached/downloaded components)
    let meta_path = path.join(".forest").join("component").join("meta.json");
    if let Ok(content) = std::fs::read_to_string(&meta_path) {
        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&content) {
            if meta.get("kind").and_then(|k| k.as_str()) == Some("deno") {
                return true;
            }
        }
    }

    // Check for TypeScript entrypoint
    path.join("src").join("main.ts").exists() && path.join("src").join("forest-sdk.ts").exists()
}

/// Get the entrypoint for a Deno component.
pub fn resolve_entrypoint(path: &Path) -> Option<String> {
    // Check meta.json for explicit entrypoint
    let meta_path = path.join(".forest").join("component").join("meta.json");
    if let Ok(content) = std::fs::read_to_string(&meta_path) {
        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(ep) = meta.get("entrypoint").and_then(|e| e.as_str()) {
                return Some(ep.to_string());
            }
        }
    }

    // Default convention
    if path.join("src").join("main.ts").exists() {
        return Some("src/main.ts".to_string());
    }

    None
}

/// Invoke a Deno component method.
///
/// Spawns `deno run` with the entrypoint and method name as argument.
/// Sends JSON payload via stdin, reads JSON response from stdout.
pub async fn invoke_deno_component(
    component_dir: &Path,
    entrypoint: &str,
    method: &str,
    spec_json: &serde_json::Value,
    input_json: &serde_json::Value,
    context: Option<&forest_sdk::CallContext>,
) -> anyhow::Result<serde_json::Value> {
    let mut payload = serde_json::json!({
        "spec": spec_json,
        "input": input_json,
    });
    if let Some(ctx) = context {
        payload["context"] = serde_json::to_value(ctx)?;
    }

    let payload_bytes = serde_json::to_vec(&payload)?;

    let entrypoint_path = component_dir.join(entrypoint);

    let mut child = tokio::process::Command::new("deno")
        .args([
            "run",
            "--allow-read",
            "--allow-env",
            "--allow-net",
            "--quiet",
            &entrypoint_path.to_string_lossy(),
            method,
        ])
        .current_dir(component_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn deno")?;

    // Write payload to stdin
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&payload_bytes).await?;
        stdin.flush().await?;
        // Drop stdin to signal EOF
    }

    let output = tokio::time::timeout(COMPONENT_TIMEOUT, child.wait_with_output())
        .await
        .context("deno component timed out")??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = stderr
            .trim()
            .strip_prefix("error: ")
            .unwrap_or(stderr.trim());
        anyhow::bail!("{msg}");
    }

    let result: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("deno component returned invalid JSON")?;

    Ok(result)
}

/// Describe a Deno component by invoking `_meta/describe`.
pub async fn describe_deno_component(
    component_dir: &Path,
    entrypoint: &str,
) -> anyhow::Result<forest_sdk::ComponentDescriptor> {
    let entrypoint_path = component_dir.join(entrypoint);

    let output = tokio::time::timeout(
        DESCRIBE_TIMEOUT,
        tokio::process::Command::new("deno")
            .args([
                "run",
                "--allow-read",
                "--allow-env",
                "--quiet",
                &entrypoint_path.to_string_lossy(),
                "_meta/describe",
            ])
            .current_dir(component_dir)
            .output(),
    )
    .await
    .context("deno describe timed out")??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("deno _meta/describe failed: {}", stderr.trim());
    }

    let descriptor: forest_sdk::ComponentDescriptor =
        serde_json::from_slice(&output.stdout).context("invalid descriptor JSON from deno")?;

    Ok(descriptor)
}

/// Load a cached descriptor from meta.json (same as binary components).
pub fn load_cached_descriptor(path: &Path) -> Option<forest_sdk::ComponentDescriptor> {
    let meta_path = path.join(".forest").join("component").join("meta.json");
    let content = std::fs::read_to_string(&meta_path).ok()?;
    let meta: serde_json::Value = serde_json::from_str(&content).ok()?;
    let descriptor = meta.get("descriptor")?;
    serde_json::from_value(descriptor.clone()).ok()
}
