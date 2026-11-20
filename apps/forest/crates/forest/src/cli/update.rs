use anyhow::Context;

use crate::{
    grpc::GrpcClientState,
    lockfile::{LockEntry, LockFile, LockSource},
    models::DependencyType,
    services::{
        component_binary,
        components::{ComponentsServiceState, EnsureCachedOutcome},
        project::ProjectParserState,
    },
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

#[derive(Default)]
struct DepUpdate {
    entry: Option<LockEntry>,
    message: Option<String>,
    updated: bool,
}

impl UpdateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let project = state.project_parser().get_project_specs().await?;
        let project_dir = project.path.clone();
        let client = state.grpc_client();
        let (os, arch) = component_binary::current_platform();

        let mut lockfile = LockFile::load(&project_dir).await?;
        let mut updated = 0;

        let mut versioned = Vec::new();
        for dep in &project.dependencies.dependencies {
            if let Some(filter) = &self.component {
                let dep_fqn = format!("{}/{}", dep.organisation, dep.name);
                if dep_fqn != *filter {
                    continue;
                }
            }

            match &dep.dependency_type {
                DependencyType::Local(path) => {
                    let version =
                        read_local_component_version(path).unwrap_or_else(|| "0.0.0".to_string());

                    let path_str = path.to_string_lossy().to_string();

                    lockfile.insert(LockEntry {
                        organisation: dep.organisation.clone(),
                        name: dep.name.clone(),
                        version,
                        source: LockSource::Path { path: path_str },
                    });
                }
                DependencyType::Versioned(current_version) => {
                    let version_str = current_version.to_string();
                    let spec = VersionSpec::parse(&version_str).with_context(|| {
                        format!(
                            "invalid version spec for {}/{}: {version_str}",
                            dep.organisation, dep.name
                        )
                    })?;
                    versioned.push((dep.clone(), spec));
                }
            }
        }

        let limiter = crate::download::Limiter::new(state.config.max_downloads_in_flight());
        let lockfile_snapshot = &lockfile;
        let results = crate::download::map_bounded(
            versioned,
            std::sync::Arc::clone(&limiter),
            |(dep, spec), lim| {
                let client = client.clone();
                async move {
                    let available = client
                        .list_component_versions(&dep.organisation, &dep.name)
                        .await
                        .with_context(|| {
                            format!("list versions of {}/{}", dep.organisation, dep.name)
                        })?;

                    let mut semver_versions: Vec<semver::Version> = available
                        .iter()
                        .filter_map(|v| semver::Version::parse(&v.version).ok())
                        .collect();
                    semver_versions.sort();

                    let resolved = spec.resolve(&semver_versions).with_context(|| {
                        format!(
                            "no published version of {}/{} matches '{spec}'",
                            dep.organisation, dep.name
                        )
                    })?;

                    let resolved_str = resolved.to_string();

                    let manifest_raw = client
                        .get_component_manifest(&dep.organisation, &dep.name, &resolved_str)
                        .await
                        .ok();
                    let is_binary = manifest_raw
                        .as_deref()
                        .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
                        .and_then(|v| {
                            v.get("kind")
                                .and_then(|k| k.as_str())
                                .map(|s| s == "binary")
                        })
                        .unwrap_or(false);

                    if is_binary {
                        if let Some(existing_hash) = lockfile_snapshot
                            .get(&dep.organisation, &dep.name, &resolved_str, os, arch)
                        {
                            let hash = existing_hash
                                .strip_prefix("sha256:")
                                .unwrap_or(existing_hash);
                            if component_binary::resolve_binary_from_hash(hash).is_some() {
                                return Ok(DepUpdate {
                                    message: Some(format!(
                                        "{}/{}@{} up to date",
                                        dep.organisation, dep.name, resolved_str
                                    )),
                                    ..Default::default()
                                });
                            }
                        }

                        let registry_os = if os == "macos" { "darwin" } else { os };
                        let label = format!(
                            "Downloading {}/{}@{}",
                            dep.organisation, dep.name, resolved_str
                        );
                        let streamed = client
                            .download_component_binary_to_file(
                                &crate::grpc::BinaryRef {
                                    organisation: &dep.organisation,
                                    name: &dep.name,
                                    version: &resolved_str,
                                    os: registry_os,
                                    arch,
                                },
                                &component_binary::binary_download_dir()?,
                                None,
                                None,
                                Some(&label),
                            )
                            .await
                            .with_context(|| {
                                format!(
                                    "failed to download {}/{}@{} ({}/{})",
                                    dep.organisation, dep.name, resolved_str, os, arch
                                )
                            })?;
                        let size_bytes = streamed.size_bytes;
                        lim.add_bytes(size_bytes);

                        let (sha256, _cache_path) =
                            component_binary::install_downloaded_binary(streamed).await?;

                        tracing::debug!(
                            "updated {}/{}@{} ({} bytes)",
                            dep.organisation,
                            dep.name,
                            resolved_str,
                            size_bytes
                        );
                        Ok(DepUpdate {
                            entry: Some(LockEntry {
                                organisation: dep.organisation.clone(),
                                name: dep.name.clone(),
                                version: resolved_str.clone(),
                                source: LockSource::Registry {
                                    os: os.to_string(),
                                    arch: arch.to_string(),
                                    sha256: format!("sha256:{sha256}"),
                                },
                            }),
                            message: Some(format!(
                                "Updated {}/{}@{}",
                                dep.organisation, dep.name, resolved_str
                            )),
                            updated: true,
                        })
                    } else {
                        let outcome = state
                            .components_service()
                            .ensure_versioned_dep_cached(
                                &dep.organisation,
                                &dep.name,
                                &resolved_str,
                            )
                            .await
                            .with_context(|| {
                                format!(
                                    "fetch files for {}/{}@{}",
                                    dep.organisation, dep.name, resolved_str
                                )
                            })?;
                        match outcome {
                            EnsureCachedOutcome::AlreadyCached => Ok(DepUpdate {
                                message: Some(format!(
                                    "{}/{}@{} up to date",
                                    dep.organisation, dep.name, resolved_str
                                )),
                                ..Default::default()
                            }),
                            EnsureCachedOutcome::Downloaded => Ok(DepUpdate {
                                message: Some(format!(
                                    "Updated {}/{}@{} (files)",
                                    dep.organisation, dep.name, resolved_str
                                )),
                                updated: true,
                                ..Default::default()
                            }),
                            EnsureCachedOutcome::BinaryRequiresPlatformDownload => {
                                tracing::warn!(
                                    "manifest probe for {}/{}@{} disagreed with ensure_versioned_dep_cached",
                                    dep.organisation,
                                    dep.name,
                                    resolved_str
                                );
                                Ok(DepUpdate::default())
                            }
                        }
                    }
                }
            },
        )
        .await;

        let mut first_error = None;
        let mut pending = Vec::new();
        for result in results {
            match result {
                Ok(update) => pending.push(update),
                Err(e) if first_error.is_none() => first_error = Some(e),
                Err(e) => tracing::warn!("update failed: {e:#}"),
            }
        }
        for update in pending {
            if let Some(entry) = update.entry {
                lockfile.insert(entry);
            }
            if let Some(message) = update.message {
                crate::ui::success(message);
            }
            if update.updated {
                updated += 1;
            }
        }

        lockfile.save(&project_dir).await?;

        if let Some(e) = first_error {
            return Err(e);
        }

        if updated > 0 {
            tracing::debug!("forest.lock written");
            crate::ui::success(format!("Updated {updated} component(s)"));
        } else {
            crate::ui::success("All components up to date");
        }

        Ok(())
    }
}

fn read_local_component_version(path: &std::path::Path) -> Option<String> {
    let meta_path = path.join(".forest").join("component").join("meta.json");
    if let Ok(content) = std::fs::read_to_string(&meta_path) {
        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(v) = meta.get("version").and_then(|v| v.as_str()) {
                return Some(v.to_string());
            }
        }
    }

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
