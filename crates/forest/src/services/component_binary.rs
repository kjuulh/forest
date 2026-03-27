use std::path::{Path, PathBuf};

/// Resolves the binary path for a v2 component.
///
/// Reads `.forest/component/meta.json` from the component directory to find the
/// SHA-256 hash of the built binary, then looks it up in the content-addressable
/// cache at `~/.cache/forest/components/bin/{sha256}`.
///
/// Returns `None` if the binary hasn't been built or isn't in the cache.
pub fn resolve_binary(component_dir: &Path, _component_name: &str) -> Option<PathBuf> {
    let meta_path = component_dir
        .join(".forest")
        .join("component")
        .join("meta.json");

    if !meta_path.exists() {
        return None;
    }

    let meta_content = std::fs::read_to_string(&meta_path).ok()?;
    let meta: serde_json::Value = serde_json::from_str(&meta_content).ok()?;

    let (os, arch) = current_platform();
    let platform_key = format!("{os}_{arch}");

    let sha256 = meta
        .get("platforms")?
        .get(&platform_key)?
        .get("sha256")?
        .as_str()?;

    resolve_binary_from_hash(sha256)
}

/// Resolves a binary from the content-addressable cache by its SHA-256 hash.
/// Path: `~/.cache/forest/components/bin/{prefix}/{sha256}`
/// where prefix = first 2 characters of the hash (like git objects).
///
/// Verifies the SHA-256 of the cached file matches the expected hash before returning.
pub fn resolve_binary_from_hash(sha256: &str) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};

    if sha256.len() < 2 {
        return None;
    }
    let cache_dir = dirs::cache_dir()?;
    let prefix = &sha256[..2];
    let binary_path = cache_dir
        .join("forest")
        .join("components")
        .join("bin")
        .join(prefix)
        .join(sha256);

    if !binary_path.exists() {
        return None;
    }

    // Verify hash integrity before trusting the cached binary
    let content = std::fs::read(&binary_path).ok()?;
    let actual_hash = hex::encode(Sha256::digest(&content));
    if actual_hash != sha256 {
        tracing::warn!(
            "cache integrity check failed for {}: expected {}, got {}. Removing.",
            binary_path.display(),
            sha256,
            actual_hash,
        );
        let _ = std::fs::remove_file(&binary_path);
        return None;
    }

    Some(binary_path)
}

/// Stores a binary in the content-addressable cache.
/// Returns the SHA-256 hash and the path where it was stored.
/// Path layout: `~/.cache/forest/components/bin/{prefix}/{sha256}`
///
/// Uses atomic write (write to temp file, then rename) to prevent
/// concurrent readers from seeing a partially written binary.
pub fn store_binary_in_cache(binary_content: &[u8]) -> anyhow::Result<(String, PathBuf)> {
    use sha2::{Digest, Sha256};

    let sha256 = hex::encode(Sha256::digest(binary_content));
    let prefix = &sha256[..2];

    let cache_dir = dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("cache dir not available"))?;
    let bin_dir = cache_dir
        .join("forest")
        .join("components")
        .join("bin")
        .join(prefix);
    std::fs::create_dir_all(&bin_dir)?;

    let binary_path = bin_dir.join(&sha256);
    if !binary_path.exists() {
        // Atomic write: write to temp file then rename
        let tmp_path = bin_dir.join(format!(".{sha256}.tmp"));
        std::fs::write(&tmp_path, binary_content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&tmp_path, &binary_path)?;
    }

    Ok((sha256, binary_path))
}

/// Check whether a directory contains a v2 component (has `forest.component.cue`).
pub fn is_v2_component(component_dir: &Path) -> bool {
    component_dir.join("forest.component.cue").exists()
}

/// Load the cached descriptor from meta.json without spawning the binary.
/// Returns None if meta.json doesn't exist or doesn't contain a descriptor.
pub fn load_cached_descriptor(
    component_dir: &Path,
) -> Option<forest_sdk::ComponentDescriptor> {
    let meta_path = component_dir
        .join(".forest")
        .join("component")
        .join("meta.json");

    let content = std::fs::read_to_string(&meta_path).ok()?;
    let meta: serde_json::Value = serde_json::from_str(&content).ok()?;

    let descriptor_val = meta.get("descriptor")?;
    serde_json::from_value(descriptor_val.clone()).ok()
}

/// Fetch template rendering config from a component binary.
pub async fn get_template_config(
    binary_path: &Path,
) -> anyhow::Result<forest_sdk::TemplateConfig> {
    let output = tokio::time::timeout(
        DESCRIBE_TIMEOUT,
        tokio::process::Command::new(binary_path)
            .arg("_meta/template_config")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("template_config timed out"))?
    .map_err(|e| anyhow::anyhow!("failed to get template config: {e}"))?;

    if !output.status.success() {
        // If the component doesn't support template_config, return defaults
        return Ok(forest_sdk::TemplateConfig::default());
    }

    let config: forest_sdk::TemplateConfig = serde_json::from_slice(&output.stdout)
        .unwrap_or_default();
    Ok(config)
}

/// Default timeout for component binary invocations.
const COMPONENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Timeout for `_meta/describe` (should be fast).
const DESCRIBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Discover available methods by running `_meta/describe` on the component binary.
pub async fn describe_component(
    binary_path: &Path,
) -> anyhow::Result<forest_sdk::ComponentDescriptor> {
    let output = tokio::time::timeout(
        DESCRIBE_TIMEOUT,
        tokio::process::Command::new(binary_path)
            .arg("_meta/describe")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!(
        "component {} timed out after {:?} on _meta/describe",
        binary_path.display(),
        DESCRIBE_TIMEOUT,
    ))?
    .map_err(|e| anyhow::anyhow!(
        "failed to spawn component {}: {e}",
        binary_path.display(),
    ))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "failed to describe component {}: {}",
            binary_path.display(),
            stderr
        );
    }

    let descriptor: forest_sdk::ComponentDescriptor = serde_json::from_slice(&output.stdout)?;
    Ok(descriptor)
}

/// Invoke a component binary method with a spec, input, and context payload.
/// Payload is passed via stdin (not CLI args) to avoid leaking secrets in process listing.
pub async fn invoke_component(
    binary_path: &Path,
    method: &str,
    spec_json: &serde_json::Value,
    input_json: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    invoke_component_with_context(binary_path, method, spec_json, input_json, None).await
}

/// Invoke a component binary method with full context.
pub async fn invoke_component_with_context(
    binary_path: &Path,
    method: &str,
    spec_json: &serde_json::Value,
    input_json: &serde_json::Value,
    context: Option<&forest_sdk::CallContext>,
) -> anyhow::Result<serde_json::Value> {
    use tokio::io::AsyncWriteExt;

    let mut payload = serde_json::json!({
        "spec": spec_json,
        "input": input_json,
    });
    if let Some(ctx) = context {
        payload["context"] = serde_json::to_value(ctx)?;
    }

    let mut child = tokio::process::Command::new(binary_path)
        .arg(method)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow::anyhow!(
            "failed to spawn component {}: {e}",
            binary_path.display(),
        ))?;

    // Write payload to stdin
    if let Some(mut stdin) = child.stdin.take() {
        let payload_bytes = serde_json::to_vec(&payload)?;
        stdin.write_all(&payload_bytes).await?;
        drop(stdin); // Close stdin to signal EOF
    }

    // Wait with timeout
    let output = tokio::time::timeout(COMPONENT_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!(
            "command '{}' timed out after {:?}",
            method,
            COMPONENT_TIMEOUT,
        ))?
        .map_err(|e| anyhow::anyhow!(
            "command '{}' failed to execute: {e}",
            method,
        ))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Extract the useful part — component stderr often has "error: <message>"
        let clean_error = stderr
            .trim()
            .strip_prefix("error: ")
            .unwrap_or(stderr.trim());
        anyhow::bail!("{clean_error}");
    }

    let result: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    Ok(result)
}

pub fn current_platform() -> (&'static str, &'static str) {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };

    let arch = if cfg!(target_arch = "x86_64") {
        "amd64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    };

    (os, arch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn component_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/components-v2")
    }

    fn binary_path() -> Option<PathBuf> {
        // Try the .forest/component/output location first (placed by build step)
        let from_output = resolve_binary(&component_dir(), "kubernetes-service");
        if from_output.is_some() {
            return from_output;
        }

        // Fall back to target/debug from workspace root
        let workspace_binary = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug/kubernetes-service");
        if workspace_binary.exists() {
            return Some(workspace_binary);
        }

        None
    }

    #[test]
    fn test_is_v2_component() {
        assert!(is_v2_component(&component_dir()));

        // v1 component should not be detected as v2
        let v1_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../components/forest/deployment");
        assert!(!is_v2_component(&v1_dir));
    }

    #[test]
    fn test_current_platform() {
        let (os, arch) = current_platform();
        assert!(
            ["linux", "macos", "windows"].contains(&os),
            "unexpected os: {os}"
        );
        assert!(
            ["amd64", "arm64"].contains(&arch),
            "unexpected arch: {arch}"
        );
    }

    #[tokio::test]
    async fn test_describe_component() {
        let Some(binary) = binary_path() else {
            eprintln!("skipping: kubernetes-service binary not built");
            return;
        };

        let descriptor = describe_component(&binary).await.unwrap();

        assert_eq!(descriptor.protocol_version, "1.1");
        assert!(!descriptor.methods.is_empty());

        // Should have commands
        let command_names: Vec<&str> = descriptor
            .methods
            .iter()
            .filter(|m| m.kind == "command")
            .map(|m| m.name.as_str())
            .collect();
        assert!(command_names.contains(&"commands/prepare"));
        assert!(command_names.contains(&"commands/status"));
        assert!(command_names.contains(&"commands/validate"));
        assert!(command_names.contains(&"commands/diff"));
        assert!(command_names.contains(&"commands/logs"));

        // Should have hooks across 3 topics
        let hook_topics: std::collections::HashSet<&str> = descriptor
            .methods
            .iter()
            .filter(|m| m.kind == "hook")
            .filter_map(|m| m.topic.as_deref())
            .collect();
        assert!(hook_topics.contains("forest/deployment"));
        assert!(hook_topics.contains("forest/observability"));
        assert!(hook_topics.contains("forest/security"));
    }

    #[tokio::test]
    async fn test_invoke_commands_status() {
        let Some(binary) = binary_path() else {
            eprintln!("skipping: kubernetes-service binary not built");
            return;
        };

        let spec = serde_json::json!({
            "name": "test-svc",
            "namespace": "default",
            "image": "test:latest",
            "environment": "dev",
            "replicas": 2,
            "resources": {"requests": {"cpu": "100m", "memory": "128Mi"}},
            "ports": [{"name": "http", "port": 8080, "protocol": "tcp", "external": true}],
            "health_checks": {"liveness": {"http": {"path": "/healthz", "port": 8080}, "initial_delay": 10, "period": 10, "timeout": 3, "failure_threshold": 3}},
            "env_vars": [],
            "labels": {},
            "annotations": {},
        });

        let result = invoke_component(
            &binary,
            "commands/status",
            &spec,
            &serde_json::json!({}),
        )
        .await
        .unwrap();

        assert_eq!(result["ready"], 2);
        assert_eq!(result["desired"], 2);
        assert_eq!(result["healthy"], true);
    }

    #[tokio::test]
    async fn test_invoke_commands_prepare_generates_manifests() {
        let Some(binary) = binary_path() else {
            eprintln!("skipping: kubernetes-service binary not built");
            return;
        };

        let spec = serde_json::json!({
            "name": "test-svc",
            "namespace": "default",
            "image": "test:latest",
            "environment": "dev",
            "replicas": 1,
            "resources": {"requests": {"cpu": "100m", "memory": "128Mi"}},
            "ports": [{"name": "http", "port": 8080, "protocol": "tcp", "external": true}],
            "health_checks": {"liveness": {"http": {"path": "/healthz", "port": 8080}, "initial_delay": 10, "period": 10, "timeout": 3, "failure_threshold": 3}},
            "env_vars": [],
            "labels": {},
            "annotations": {},
        });

        let result = invoke_component(
            &binary,
            "commands/prepare",
            &spec,
            &serde_json::json!({}),
        )
        .await
        .unwrap();

        let manifests = result["manifests"].as_array().expect("manifests array");
        assert!(manifests.len() >= 2, "expected at least Deployment + Service");

        let deployment = manifests[0].as_str().unwrap();
        assert!(deployment.contains("kind: Deployment"));
        assert!(deployment.contains("name: test-svc"));
        assert!(deployment.contains("image: test:latest"));

        let service = manifests[1].as_str().unwrap();
        assert!(service.contains("kind: Service"));
        assert!(service.contains("port: 8080"));
    }

    #[tokio::test]
    async fn test_invoke_commands_validate() {
        let Some(binary) = binary_path() else {
            eprintln!("skipping: kubernetes-service binary not built");
            return;
        };

        let spec = serde_json::json!({
            "name": "test-svc",
            "namespace": "default",
            "image": "test:latest",
            "environment": "dev",
            "replicas": 1,
            "resources": {"requests": {"cpu": "100m", "memory": "128Mi"}},
            "ports": [{"name": "http", "port": 8080, "protocol": "tcp", "external": true}],
            "health_checks": {"liveness": {"http": {"path": "/healthz", "port": 8080}, "initial_delay": 10, "period": 10, "timeout": 3, "failure_threshold": 3}},
            "env_vars": [],
            "labels": {},
            "annotations": {},
        });

        let result = invoke_component(
            &binary,
            "commands/validate",
            &spec,
            &serde_json::json!({}),
        )
        .await
        .unwrap();

        assert_eq!(result["valid"], true);
        assert!(result["errors"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_invoke_hook_deployment_prepare() {
        let Some(binary) = binary_path() else {
            eprintln!("skipping: kubernetes-service binary not built");
            return;
        };

        let spec = serde_json::json!({
            "name": "test-svc",
            "namespace": "default",
            "image": "test:latest",
            "environment": "dev",
            "replicas": 1,
            "resources": {"requests": {"cpu": "100m", "memory": "128Mi"}},
            "ports": [],
            "health_checks": {"liveness": {"http": {"path": "/healthz", "port": 8080}, "initial_delay": 10, "period": 10, "timeout": 3, "failure_threshold": 3}},
            "env_vars": [],
            "labels": {},
            "annotations": {},
        });

        // Hook should succeed without error
        let result = invoke_component(
            &binary,
            "hooks/forest/deployment/prepare",
            &spec,
            &serde_json::json!({}),
        )
        .await
        .unwrap();

        // Hook returns empty output (side effects only)
        assert!(result.is_object() || result.is_null());
    }

    #[tokio::test]
    async fn test_invoke_hook_security_scan_image() {
        let Some(binary) = binary_path() else {
            eprintln!("skipping: kubernetes-service binary not built");
            return;
        };

        let spec = serde_json::json!({
            "name": "test-svc",
            "namespace": "default",
            "image": "test:latest",
            "environment": "dev",
            "replicas": 1,
            "resources": {"requests": {"cpu": "100m", "memory": "128Mi"}},
            "ports": [],
            "health_checks": {"liveness": {"http": {"path": "/healthz", "port": 8080}, "initial_delay": 10, "period": 10, "timeout": 3, "failure_threshold": 3}},
            "env_vars": [],
            "labels": {},
            "annotations": {},
        });

        let result = invoke_component(
            &binary,
            "hooks/forest/security/scan_image",
            &spec,
            &serde_json::json!({}),
        )
        .await
        .unwrap();

        assert!(result["passed"].as_bool().unwrap());
        assert_eq!(result["critical"], 0);
    }
}
