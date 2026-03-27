use anyhow::Context;
use sha2::{Digest, Sha256};

use crate::{
    grpc::GrpcClientState,
    services::component_binary,
    state::State,
};

/// Publish the component to the Forest registry.
///
/// Uploads the compiled binary, CUE spec files (forest.cue,
/// forest.component.cue, spec.cue), and the component manifest
/// to the registry. Requires `forest build` to be run first.
///
/// The component is published under {organisation}/{name}@{version}
/// as declared in forest.cue. Requires org membership.
#[derive(clap::Parser)]
pub struct PublishCommand {}

impl PublishCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        // 1. Parse the component's CUE files to get metadata
        let mut cue_args = vec!["export".to_string(), "--out".to_string(), "json".to_string()];
        let current_dir = std::env::current_dir()?;
        // Collect all .cue files for evaluation
        let mut dir_entries = tokio::fs::read_dir(&current_dir).await?;
        while let Some(entry) = dir_entries.next_entry().await? {
            if entry.path().extension().and_then(|e| e.to_str()) == Some("cue") {
                cue_args.push(entry.file_name().to_string_lossy().to_string());
            }
        }

        let mut cmd = tokio::process::Command::new("cue");
        cmd.args(&cue_args);
        if let Ok(registry) = std::env::var("CUE_REGISTRY") {
            cmd.env("CUE_REGISTRY", registry);
        }
        let output = cmd.output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("failed to parse component CUE: {stderr}");
        }

        let doc: serde_json::Value = serde_json::from_slice(&output.stdout)?;

        // Extract metadata — forest.component is optional for CUE-only components
        let component = doc
            .get("forest")
            .and_then(|f| f.get("component"));

        let project = doc.get("project");

        let name = component
            .and_then(|c| c.get("name"))
            .and_then(|v| v.as_str())
            .or_else(|| project.and_then(|p| p.get("name")).and_then(|v| v.as_str()))
            .context("component or project name is required")?;

        let version = component
            .and_then(|c| c.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or("0.1.0");

        let organisation = project
            .and_then(|p| p.get("organisation"))
            .and_then(|v| v.as_str())
            .context("project.organisation is required")?;

        tracing::info!(
            "publishing component {organisation}/{name}@{version}"
        );

        // 2. Check for binary (optional — CUE-only components don't need one)
        let binary_path = component_binary::resolve_binary(&current_dir, name);

        let (descriptor, kind) = if let Some(ref bp) = binary_path {
            let desc = if let Some(cached) = component_binary::load_cached_descriptor(&current_dir)
            {
                cached
            } else {
                component_binary::describe_component(bp).await?
            };
            (Some(desc), "binary")
        } else {
            (None, "cue")
        };

        // 3. Build manifest
        let mut manifest = serde_json::json!({
            "name": name,
            "organisation": organisation,
            "version": version,
            "kind": kind,
        });

        if let Some(ref desc) = descriptor {
            manifest["protocol_version"] = serde_json::json!(desc.protocol_version);
            manifest["capabilities"] = serde_json::json!({ "methods": desc.methods });

            let (os, arch) = component_binary::current_platform();
            let binary_content = tokio::fs::read(binary_path.as_ref().unwrap()).await?;
            let sha256 = hex::encode(Sha256::digest(&binary_content));
            manifest["platforms"] = serde_json::json!({
                format!("{os}_{arch}"): {
                    "sha256": sha256,
                    "size": binary_content.len(),
                }
            });
        }

        tracing::info!(
            "manifest: kind={}, {}",
            kind,
            descriptor
                .as_ref()
                .map(|d| format!("{} methods", d.methods.len()))
                .unwrap_or_else(|| "CUE-only (no binary)".to_string()),
        );

        // 4. Begin upload
        let client = state.grpc_client();
        tracing::info!("beginning upload");
        let upload_context = client
            .begin_component_upload(organisation, name, version)
            .await?;

        // 5. Upload binary (if present)
        if let Some(ref bp) = binary_path {
            let (os, arch) = component_binary::current_platform();
            let binary_content = tokio::fs::read(bp).await?;
            let sha256 = hex::encode(Sha256::digest(&binary_content));
            tracing::info!("uploading binary ({} bytes)", binary_content.len());
            client
                .upload_component_binary(&upload_context, os, arch, &sha256, &binary_content)
                .await?;
        }

        // 6. Upload CUE spec files
        let cue_files: Vec<(String, String)> = collect_cue_files(&current_dir).await?;
        if !cue_files.is_empty() {
            tracing::info!("uploading {} CUE spec file(s)", cue_files.len());
            for (rel_path, content) in &cue_files {
                client
                    .upload_component_file(&upload_context, rel_path, content.as_bytes())
                    .await
                    .with_context(|| format!("upload CUE file: {rel_path}"))?;
            }
        }

        // 7. Publish manifest
        tracing::info!("publishing manifest");
        let manifest_json = serde_json::to_string(&manifest)?;
        client
            .publish_component_manifest(&upload_context, &manifest_json)
            .await?;

        // 8. Commit
        tracing::info!("committing upload");
        client.commit_component_upload(&upload_context).await?;

        tracing::info!("published {organisation}/{name}@{version} successfully");

        Ok(())
    }
}

/// Collect all `.cue` files from a directory (non-recursive, excludes cue.mod/).
async fn collect_cue_files(
    dir: &std::path::Path,
) -> anyhow::Result<Vec<(String, String)>> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;

    // Only include component-relevant CUE files (forest.cue, forest.component.cue, spec.cue, *.schema.cue)
    // Skip consumer examples, tests, etc.
    let component_cue_files = ["forest.cue", "forest.component.cue", "spec.cue"];

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("cue") {
            let file_name = entry.file_name().to_string_lossy().to_string();
            if component_cue_files.contains(&file_name.as_str())
                || file_name.ends_with(".schema.cue")
            {
                let content = tokio::fs::read_to_string(&path).await?;
                files.push((file_name, content));
            }
        }
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}
