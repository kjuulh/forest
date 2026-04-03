//! Deno/TypeScript component invocation.
//!
//! Implements Forest component protocol v2 with streaming JSON lines.
//! Components can call other components during execution via call/call_result
//! message pairs, mediated by the runtime.

use std::path::Path;

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use std::time::Duration;

const COMPONENT_TIMEOUT: Duration = Duration::from_secs(120);
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Callback for resolving inter-component calls.
/// Given a component identifier, method, spec, and input, invokes the target
/// component and returns its result.
pub type ComponentCallResolver = Box<
    dyn Fn(
            String,                           // component identifier
            String,                           // method
            serde_json::Value,                // spec
            serde_json::Value,                // input
            Option<forest_sdk::CallContext>,   // context from the caller
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<serde_json::Value>> + Send>>
        + Send
        + Sync,
>;

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
pub fn is_deno_component(path: &Path) -> bool {
    is_deno_component_with_meta(path, None, None, None)
}

/// Detect if a component directory contains a Deno component,
/// checking the shared cache if org/name/version are provided.
pub fn is_deno_component_with_meta(
    path: &Path,
    organisation: Option<&str>,
    name: Option<&str>,
    version: Option<&str>,
) -> bool {
    if let Some(meta) = read_meta_json(path, organisation, name, version) {
        if meta.get("kind").and_then(|k| k.as_str()) == Some("deno") {
            return true;
        }
    }
    path.join("src").join("main.ts").exists() && path.join("src").join("forest-sdk.ts").exists()
}

/// Get the entrypoint for a Deno component.
pub fn resolve_entrypoint(path: &Path) -> Option<String> {
    resolve_entrypoint_with_meta(path, None, None, None)
}

/// Get the entrypoint for a Deno component,
/// checking the shared cache if org/name/version are provided.
pub fn resolve_entrypoint_with_meta(
    path: &Path,
    organisation: Option<&str>,
    name: Option<&str>,
    version: Option<&str>,
) -> Option<String> {
    if let Some(meta) = read_meta_json(path, organisation, name, version) {
        if let Some(ep) = meta.get("entrypoint").and_then(|e| e.as_str()) {
            return Some(ep.to_string());
        }
    }
    if path.join("src").join("main.ts").exists() {
        return Some("src/main.ts".to_string());
    }
    None
}

/// Read meta.json from the shared cache or local `.forest/` directory.
fn read_meta_json(
    path: &Path,
    organisation: Option<&str>,
    name: Option<&str>,
    version: Option<&str>,
) -> Option<serde_json::Value> {
    let meta_path = if let (Some(org), Some(n), Some(v)) = (organisation, name, version) {
        super::component_binary::resolve_meta_json(path, org, n, v)?
    } else {
        let local = path.join(".forest").join("component").join("meta.json");
        if !local.exists() { return None; }
        local
    };
    let content = std::fs::read_to_string(&meta_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Invoke a Deno component method using protocol v2.
///
/// Spawns the component, sends an invoke message, then enters a loop
/// handling call/return messages until the component returns its final result.
pub async fn invoke_deno_component(
    component_dir: &Path,
    entrypoint: &str,
    method: &str,
    spec_json: &serde_json::Value,
    input_json: &serde_json::Value,
    context: Option<&forest_sdk::CallContext>,
    call_resolver: Option<&ComponentCallResolver>,
) -> anyhow::Result<serde_json::Value> {
    tracing::trace!(
        component_dir = %component_dir.display(),
        entrypoint = %entrypoint,
        method = %method,
        "rpc call → deno component"
    );
    tracing::trace!(spec = %spec_json, input = %input_json, "rpc request payload");

    let invoke_msg = serde_json::json!({
        "type": "invoke",
        "method": method,
        "spec": spec_json,
        "input": input_json,
        "context": context.map(|c| serde_json::to_value(c).unwrap_or_default())
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
    });

    let component_dir = component_dir.canonicalize()
        .with_context(|| format!("canonicalize component dir: {}", component_dir.display()))?;
    let entrypoint_path = component_dir.join(entrypoint);

    let run_dir = context
        .and_then(|ctx| ctx.work_dir.as_deref())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| component_dir.clone());

    let mut child = tokio::process::Command::new("deno")
        .args([
            "run",
            "--allow-read",
            "--allow-write",
            "--allow-env",
            "--allow-net",
            "--allow-run",
            "--quiet",
            &entrypoint_path.to_string_lossy(),
            method,
        ])
        .current_dir(run_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn deno")?;

    let mut stdin = child.stdin.take().context("failed to get stdin")?;
    let stdout = child.stdout.take().context("failed to get stdout")?;
    let stderr = child.stderr.take().context("failed to get stderr")?;

    // Send invoke message
    let invoke_line = serde_json::to_string(&invoke_msg)? + "\n";
    stdin.write_all(invoke_line.as_bytes()).await?;
    stdin.flush().await?;

    // Stream stderr to tracing in background
    let stderr_handle = tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::info!(target: "component", "{}", line);
        }
    });

    // Read stdout lines, handling call/return protocol
    let mut stdout_reader = BufReader::new(stdout);
    let result = tokio::time::timeout(COMPONENT_TIMEOUT, async {
        loop {
            let mut line = String::new();
            let bytes_read = stdout_reader.read_line(&mut line).await
                .context("read stdout line")?;
            if bytes_read == 0 {
                anyhow::bail!("component closed stdout without returning a result");
            }

            let msg: serde_json::Value = serde_json::from_str(line.trim())
                .with_context(|| format!("invalid JSON line from component: {}", line.trim()))?;

            match msg.get("type").and_then(|t| t.as_str()) {
                Some("return") => {
                    let result = msg.get("result")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    tracing::trace!(method = %method, result = %result, "rpc response ← deno component");
                    return Ok(result);
                }
                Some("call") => {
                    let component = msg.get("component").and_then(|c| c.as_str())
                        .context("call message missing 'component'")?
                        .to_string();
                    let call_method = msg.get("method").and_then(|m| m.as_str())
                        .context("call message missing 'method'")?
                        .to_string();
                    tracing::trace!(
                        component = %component,
                        call_method = %call_method,
                        "rpc inter-component call"
                    );
                    let call_id = msg.get("id").and_then(|i| i.as_str())
                        .unwrap_or("0")
                        .to_string();
                    let call_spec = msg.get("spec").cloned()
                        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                    let call_input = msg.get("input").cloned()
                        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                    let call_context: Option<forest_sdk::CallContext> = msg.get("context")
                        .and_then(|c| serde_json::from_value(c.clone()).ok());

                    let call_result = if let Some(resolver) = call_resolver {
                        match resolver(component.clone(), call_method.clone(), call_spec, call_input, call_context).await {
                            Ok(result) => result,
                            Err(e) => {
                                tracing::error!("call to {component}/{call_method} failed: {e}");
                                serde_json::Value::Null
                            }
                        }
                    } else {
                        tracing::warn!("component requested call to {component}/{call_method} but no resolver available");
                        serde_json::Value::Null
                    };

                    let response = serde_json::json!({
                        "type": "call_result",
                        "id": call_id,
                        "result": call_result,
                    });
                    let response_line = serde_json::to_string(&response)? + "\n";
                    stdin.write_all(response_line.as_bytes()).await?;
                    stdin.flush().await?;
                }
                other => {
                    anyhow::bail!("unexpected message type from component: {:?}", other);
                }
            }
        }
    })
    .await
    .context("deno component timed out")??;

    // Wait for process to finish
    drop(stdin);
    let _ = child.wait().await;
    stderr_handle.abort();

    Ok(result)
}

/// Invoke without callback support (for simple cases).
pub async fn invoke_deno_component_simple(
    component_dir: &Path,
    entrypoint: &str,
    method: &str,
    spec_json: &serde_json::Value,
    input_json: &serde_json::Value,
    context: Option<&forest_sdk::CallContext>,
) -> anyhow::Result<serde_json::Value> {
    invoke_deno_component(
        component_dir, entrypoint, method,
        spec_json, input_json, context, None,
    ).await
}

/// Describe a Deno component by invoking `_meta/describe`.
pub async fn describe_deno_component(
    component_dir: &Path,
    entrypoint: &str,
) -> anyhow::Result<forest_sdk::ComponentDescriptor> {
    let component_dir = component_dir.canonicalize()
        .with_context(|| format!("canonicalize component dir: {}", component_dir.display()))?;
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

/// Load a cached descriptor from meta.json.
pub fn load_cached_descriptor(path: &Path) -> Option<forest_sdk::ComponentDescriptor> {
    load_cached_descriptor_with_meta(path, None, None, None)
}

/// Load a cached descriptor from meta.json, checking the shared cache if org/name/version are provided.
pub fn load_cached_descriptor_with_meta(
    path: &Path,
    organisation: Option<&str>,
    name: Option<&str>,
    version: Option<&str>,
) -> Option<forest_sdk::ComponentDescriptor> {
    let meta = read_meta_json(path, organisation, name, version)?;
    let descriptor = meta.get("descriptor")?;
    serde_json::from_value(descriptor.clone()).ok()
}
