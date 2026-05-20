use anyhow::Context;
use forest_grpc_interface::ProjectMetadata;
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

        // Sync project-level metadata (description, About-sidebar fields, README)
        // from forest.cue → server. CUE is source of truth: missing in CUE = cleared.
        // See specs/features/009-project-metadata.md.
        sync_project_fields(state, &current_dir, organisation, name, &doc).await?;

        // Dispatch: `external:` block in forest.cue means external manifest mode
        // (TASKS/018-global-tools.md §1a.2b). No build, no UploadBinary.
        let external = component.and_then(|c| c.get("external"));
        if let Some(external_block) = external {
            return publish_external(
                state,
                &current_dir,
                organisation,
                name,
                version,
                &doc,
                external_block,
            )
            .await;
        }

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
            // Methods are also surfaced as a plain string array for the
            // shape derivation in forest-manifest (HYBRID vs COMPONENT).
            let method_names: Vec<String> = desc.methods.iter().map(|m| m.name.clone()).collect();
            manifest["methods"] = serde_json::json!(method_names);
            manifest["capabilities"] = serde_json::json!({ "methods": desc.methods });
            // Carry the tool facet through to the published manifest if the
            // describe response advertised one.
            if let Some(tool) = describe_response_tool_facet(desc) {
                manifest["tool"] = tool;
            }

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

/// Ensure the project exists and push its declared fields (description,
/// metadata, README) up to the server before the artefact upload.
///
/// - Calls `create_project` first (idempotent: server upserts on conflict)
///   so a publish into a brand-new project still works without a separate
///   `forest project create` step.
/// - Reads `project.description` and `project.metadata.*` from the
///   already-parsed CUE JSON.
/// - Reads README.md from the project directory if present.
/// - Sends all three to `UpdateProject` with field-mask semantics — empty
///   values clear the server. See spec §"Publish flow".
async fn sync_project_fields(
    state: &State,
    current_dir: &std::path::Path,
    organisation: &str,
    name: &str,
    doc: &serde_json::Value,
) -> anyhow::Result<()> {
    let client = state.grpc_client();

    // Idempotent — server treats existing project as a no-op via ON CONFLICT.
    client
        .create_project(organisation, name)
        .await
        .context("ensure project exists")?;

    let project = doc.get("project");

    // String fields default to "" when missing from CUE (= clear server-side).
    let description = project
        .and_then(|p| p.get("description"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let metadata = project
        .and_then(|p| p.get("metadata"))
        .map(extract_project_metadata)
        .unwrap_or_default();

    let readme = read_optional_readme(current_dir).await?;

    client
        .update_project(
            organisation,
            name,
            Some(readme),
            Some(description),
            Some(metadata),
        )
        .await
        .context("push project description + metadata + readme")?;

    Ok(())
}

/// Pull blessed metadata fields out of the parsed CUE JSON.
/// Missing keys become empty strings (cleared server-side per spec).
fn extract_project_metadata(meta: &serde_json::Value) -> ProjectMetadata {
    let s = |key: &str| -> String {
        meta.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    ProjectMetadata {
        git_url: s("git_url"),
        homepage: s("homepage"),
        docs_url: s("docs_url"),
        support_url: s("support_url"),
        domain: s("domain"),
        owner: s("owner"),
    }
}

/// Read a project's README.md (case-insensitive) if present. Returns
/// empty string when absent — server treats that as "clear", matching
/// the missing-in-CUE-clears policy.
async fn read_optional_readme(current_dir: &std::path::Path) -> anyhow::Result<String> {
    for candidate in ["README.md", "Readme.md", "readme.md"] {
        let p = current_dir.join(candidate);
        match tokio::fs::read_to_string(&p).await {
            Ok(contents) => return Ok(contents),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
        }
    }
    Ok(String::new())
}

/// Read the optional `tool` facet from a component's `_meta/describe`
/// response if it advertised one. Returns the JSON form ready to embed
/// in the manifest. The describe protocol places `tool` alongside
/// `methods` (see TASKS/018-global-tools.md §1a.1).
fn describe_response_tool_facet(
    desc: &forest_sdk::ComponentDescriptor,
) -> Option<serde_json::Value> {
    desc.tool.as_ref().map(|t| {
        let mut obj = serde_json::json!({
            "name": t.name,
            "argv_passthrough": t.argv_passthrough,
        });
        if let Some(d) = &t.description {
            obj["description"] = serde_json::json!(d);
        }
        obj
    })
}

/// External-manifest publishing path. Skips the binary build/upload entirely
/// and submits only the manifest (kind=external). See §1a.2b.
async fn publish_external(
    state: &State,
    current_dir: &std::path::Path,
    organisation: &str,
    name: &str,
    version: &str,
    doc: &serde_json::Value,
    external_block: &serde_json::Value,
) -> anyhow::Result<()> {
    // Build the platforms map from the CUE `external.platforms` array.
    let raw_platforms = external_block
        .get("platforms")
        .and_then(|v| v.as_array())
        .context("forest.component.external.platforms must be an array")?;

    let mut platforms = serde_json::Map::new();
    for entry in raw_platforms {
        let os = entry
            .get("os")
            .and_then(|v| v.as_str())
            .context("platform entry missing `os`")?;
        let arch = entry
            .get("arch")
            .and_then(|v| v.as_str())
            .context("platform entry missing `arch`")?;
        let sha256 = entry
            .get("sha256")
            .and_then(|v| v.as_str())
            .context("platform entry missing `sha256`")?;
        let url = entry
            .get("url")
            .and_then(|v| v.as_str())
            .context("platform entry missing `url`")?;
        let archive = entry
            .get("archive")
            .and_then(|v| v.as_str())
            .unwrap_or("none");

        let mut platform_obj = serde_json::json!({
            "sha256": sha256,
            "url": url,
            "archive": archive,
        });
        if let Some(b) = entry.get("binary_in_archive").and_then(|v| v.as_str()) {
            platform_obj["binary_in_archive"] = serde_json::json!(b);
        }
        if let Some(a) = entry.get("archive_sha256").and_then(|v| v.as_str()) {
            platform_obj["archive_sha256"] = serde_json::json!(a);
        }
        platforms.insert(format!("{os}_{arch}"), platform_obj);
    }

    // Extract the `#Tool` facet via a dedicated `cue eval -e tool`.
    // `#Tool` is a CUE definition (hidden from `cue export`); we eval it
    // explicitly to extract its concrete JSON form.
    let tool_facet = eval_tool_facet(current_dir).await?;

    let manifest = serde_json::json!({
        "name": name,
        "organisation": organisation,
        "version": version,
        "kind": "external",
        "tool": tool_facet,
        "platforms": platforms,
    });

    tracing::info!(
        "publishing external manifest: {organisation}/{name}@{version} ({} platforms)",
        platforms.len()
    );
    let _ = doc; // reserved for future fields

    let client = state.grpc_client();
    let upload_context = client
        .begin_component_upload(organisation, name, version)
        .await?;

    // Skip UploadBinary entirely — externals are URL-hosted.
    // Upload the CUE files (lightweight, for the registry's discovery UI).
    let cue_files: Vec<(String, String)> = collect_cue_files(current_dir).await?;
    for (rel_path, content) in &cue_files {
        client
            .upload_component_file(&upload_context, rel_path, content.as_bytes())
            .await
            .with_context(|| format!("upload CUE file: {rel_path}"))?;
    }

    let manifest_json = serde_json::to_string(&manifest)?;
    client
        .publish_component_manifest(&upload_context, &manifest_json)
        .await?;
    client.commit_component_upload(&upload_context).await?;

    tracing::info!(
        "published external tool {organisation}/{name}@{version} (kind=external)"
    );
    Ok(())
}

/// Evaluate `#Tool` from the project's CUE package. Since `#Tool` is a
/// definition (hidden from `cue export`), we use `cue eval --expression`
/// to extract its concrete value.
async fn eval_tool_facet(dir: &std::path::Path) -> anyhow::Result<serde_json::Value> {
    let mut cmd = tokio::process::Command::new("cue");
    cmd.current_dir(dir)
        .args(["eval", "--out=json", "-e", "#Tool", "."]);
    if let Ok(registry) = std::env::var("CUE_REGISTRY") {
        cmd.env("CUE_REGISTRY", registry);
    }
    let output = cmd
        .output()
        .await
        .context("running `cue eval -e #Tool`")?;
    if !output.status.success() {
        anyhow::bail!(
            "cue eval -e #Tool failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let v: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("parsing cue eval -e #Tool output")?;
    Ok(v)
}

/// Collect all `.cue` files from a directory (non-recursive, excludes cue.mod/).
async fn collect_cue_files(
    dir: &std::path::Path,
) -> anyhow::Result<Vec<(String, String)>> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;

    // Include all .cue files in the component directory.
    // These form the component's public API (types, contracts, specs).
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("cue") {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let content = tokio::fs::read_to_string(&path).await?;
            files.push((file_name, content));
        }
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}
