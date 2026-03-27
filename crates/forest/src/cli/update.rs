use anyhow::Context;

use crate::{
    grpc::GrpcClientState,
    lockfile::{LockEntry, LockFile, LockSource},
    models::DependencyType,
    services::{component_binary, project::ProjectParserState},
    state::State,
    version_spec::VersionSpec,
};

/// Update dependencies to the latest versions matching the spec.
///
/// Resolves each versioned dependency against the registry, finds the
/// highest version matching the version spec (e.g., "0.1" → latest 0.1.x),
/// downloads the binary, and updates forest.lock.
///
/// Local path dependencies are also recorded in forest.lock (with their
/// path and version), but always resolve from disk.
///
/// Examples:
///   forest update                    # update all deps
///   forest update forest-contrib/kubernetes-service  # update one dep
#[derive(clap::Parser)]
pub struct UpdateCommand {
    /// Specific component to update (org/name). If omitted, updates all.
    component: Option<String>,
}

impl UpdateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let project = state.project_parser().get_project().await?;
        let project_dir = project.path.clone();
        let client = state.grpc_client();
        let (os, arch) = component_binary::current_platform();

        let mut lockfile = LockFile::load(&project_dir).await?;
        let mut updated = 0;

        for dep in &project.dependencies.dependencies {
            // Filter to specific component if requested
            if let Some(filter) = &self.component {
                let dep_fqn = format!("{}/{}", dep.organisation, dep.name);
                if dep_fqn != *filter {
                    continue;
                }
            }

            match &dep.dependency_type {
                DependencyType::Local(path) => {
                    // Record path dep in lock file (like Cargo does).
                    // The version is read from the component's CUE config.
                    let version = read_local_component_version(path)
                        .unwrap_or_else(|| "0.0.0".to_string());

                    let path_str = path.to_string_lossy().to_string();

                    lockfile.insert(LockEntry {
                        organisation: dep.organisation.clone(),
                        name: dep.name.clone(),
                        version,
                        source: LockSource::Path { path: path_str },
                    });
                }
                DependencyType::Versioned(current_version) => {
                    // The version in forest.cue is the spec (e.g., "0.1" or "1" or "0.1.0")
                    let version_str = current_version.to_string();
                    let spec = VersionSpec::parse(&version_str).with_context(|| {
                        format!(
                            "invalid version spec for {}/{}: {version_str}",
                            dep.organisation, dep.name
                        )
                    })?;

                    // List available versions from registry
                    let versions_response = client
                        .list_component_versions(&dep.organisation, &dep.name)
                        .await;

                    let available = match versions_response {
                        Ok(resp) => resp,
                        Err(e) => {
                            tracing::warn!(
                                "failed to list versions for {}/{}: {e}",
                                dep.organisation, dep.name
                            );
                            continue;
                        }
                    };

                    // Parse available versions
                    let mut semver_versions: Vec<semver::Version> = available
                        .iter()
                        .filter_map(|v| semver::Version::parse(&v.version).ok())
                        .collect();
                    semver_versions.sort();

                    // Resolve the best match
                    let Some(resolved) = spec.resolve(&semver_versions) else {
                        println!(
                            "  {} {}/{}  no version matches spec '{spec}'",
                            "!", dep.organisation, dep.name
                        );
                        continue;
                    };

                    let resolved_str = resolved.to_string();

                    // Check if we already have this version cached
                    if let Some(existing_hash) = lockfile.get(
                        &dep.organisation,
                        &dep.name,
                        &resolved_str,
                        os,
                        arch,
                    ) {
                        // Already locked at this version — check if binary is in cache
                        let hash =
                            existing_hash.strip_prefix("sha256:").unwrap_or(existing_hash);
                        if component_binary::resolve_binary_from_hash(hash).is_some() {
                            println!(
                                "  {} {}/{}@{}  up to date",
                                "✓", dep.organisation, dep.name, resolved_str
                            );
                            continue;
                        }
                    }

                    // Download the binary
                    println!(
                        "  {} {}/{}@{}  downloading...",
                        "↓", dep.organisation, dep.name, resolved_str
                    );

                    let binary = client
                        .download_component_binary(
                            &dep.organisation,
                            &dep.name,
                            &resolved_str,
                            os,
                            arch,
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "failed to download {}/{}@{} ({}/{})",
                                dep.organisation, dep.name, resolved_str, os, arch
                            )
                        })?;

                    let (sha256, _cache_path) =
                        component_binary::store_binary_in_cache(&binary)?;

                    lockfile.insert(LockEntry {
                        organisation: dep.organisation.clone(),
                        name: dep.name.clone(),
                        version: resolved_str.clone(),
                        source: LockSource::Registry {
                            os: os.to_string(),
                            arch: arch.to_string(),
                            sha256: format!("sha256:{sha256}"),
                        },
                    });

                    println!(
                        "  {} {}/{}@{}  updated ({} bytes)",
                        "✓", dep.organisation, dep.name, resolved_str,
                        binary.len()
                    );
                    updated += 1;
                }
            }
        }

        lockfile.save(&project_dir).await?;

        println!();
        if updated > 0 {
            println!("Updated {updated} component(s). forest.lock written.");
        } else {
            println!("All components up to date.");
        }

        Ok(())
    }
}

/// Read the version from a local component's CUE config (forest.cue).
/// Returns None if the version can't be determined.
fn read_local_component_version(path: &std::path::Path) -> Option<String> {
    // Try to read from .forest/component/meta.json first
    let meta_path = path.join(".forest").join("component").join("meta.json");
    if let Ok(content) = std::fs::read_to_string(&meta_path) {
        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(v) = meta.get("version").and_then(|v| v.as_str()) {
                return Some(v.to_string());
            }
        }
    }

    // Fallback: try running cue export to get the version
    let output = std::process::Command::new("cue")
        .args(["export", "--out", "json", "forest.cue"])
        .current_dir(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    doc.get("forest")
        .and_then(|f| f.get("component"))
        .and_then(|c| c.get("version"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}
