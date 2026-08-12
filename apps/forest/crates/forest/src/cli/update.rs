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

/// What updating one versioned dependency produced.
///
/// Dependencies are resolved and downloaded concurrently (DATA-505), so the
/// per-dep work cannot write `forest.lock` or print progress lines itself —
/// interleaved output is unreadable and concurrent read-modify-write of the
/// lockfile loses entries. Each unit of work returns this instead, and the
/// caller applies it in dependency order once the fan-out has drained.
#[derive(Default)]
struct DepUpdate {
    /// Lockfile entry to record, if any.
    entry: Option<LockEntry>,
    /// Line to show the user, replayed in dependency order.
    message: Option<UpdateMessage>,
    /// Whether this counts toward the "updated N component(s)" tally.
    updated: bool,
}

enum UpdateMessage {
    Success(String),
    Warn(String),
}

impl UpdateMessage {
    fn emit(self) {
        match self {
            UpdateMessage::Success(m) => crate::ui::success(m),
            UpdateMessage::Warn(m) => crate::ui::warn(m),
        }
    }
}

impl UpdateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let project = state.project_parser().get_project().await?;
        let project_dir = project.path.clone();
        let client = state.grpc_client();
        let (os, arch) = component_binary::current_platform();

        let mut lockfile = LockFile::load(&project_dir).await?;
        let mut updated = 0;

        // Pass 1 (serial, no I/O worth parallelising): local path deps are
        // recorded straight away, versioned deps are collected for the
        // concurrent pass.
        let mut versioned = Vec::new();
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
                    // The version in forest.cue is the spec (e.g., "0.1" or "1" or "0.1.0")
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

        // Pass 2 (concurrent, bounded + adaptive): resolve each spec against
        // the registry and fetch what it points at. `lockfile` is read-only
        // here — the up-to-date check only reads Registry entries, and the
        // path-dep inserts above never touch those.
        let limiter = crate::download::Limiter::new(state.config.max_downloads_in_flight());
        let lockfile_snapshot = &lockfile;
        let results = crate::download::map_bounded(
            versioned,
            std::sync::Arc::clone(&limiter),
            |(dep, spec), lim| {
                let client = client.clone();
                async move {
                    // List available versions from registry
                    let versions_response = client
                        .list_component_versions(&dep.organisation, &dep.name)
                        .await;

                    let available = match versions_response {
                        Ok(resp) => resp,
                        Err(e) => {
                            tracing::warn!(
                                "failed to list versions for {}/{}: {e}",
                                dep.organisation,
                                dep.name
                            );
                            return Ok(DepUpdate::default());
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
                        return Ok(DepUpdate {
                            message: Some(UpdateMessage::Warn(format!(
                                "{}/{}: no version matches '{spec}'",
                                dep.organisation, dep.name
                            ))),
                            ..Default::default()
                        });
                    };

                    let resolved_str = resolved.to_string();

                    // Component kind drives the download path:
                    //   - kind=binary → per-platform binary download + lockfile hash
                    //   - kind=cue / deno / files → stream the file bundle into
                    //     the cache via the shared ensure_versioned_dep_cached
                    //     helper; no lockfile hash (immutable version is the lock)
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
                        // Check if we already have this version cached
                        if let Some(existing_hash) = lockfile_snapshot
                            .get(&dep.organisation, &dep.name, &resolved_str, os, arch)
                        {
                            let hash = existing_hash
                                .strip_prefix("sha256:")
                                .unwrap_or(existing_hash);
                            if component_binary::resolve_binary_from_hash(hash).is_some() {
                                return Ok(DepUpdate {
                                    message: Some(UpdateMessage::Success(format!(
                                        "{}/{}@{} up to date",
                                        dep.organisation, dep.name, resolved_str
                                    ))),
                                    ..Default::default()
                                });
                            }
                        }

                        // The registry stores macOS binaries under the "darwin"
                        // os key (publish translates macos→darwin on upload).
                        // `forest run`'s download path does this too; the
                        // update path needs the same translation or a macOS
                        // consumer gets a spurious "binary not found". DATA-312.
                        let registry_os = if os == "macos" { "darwin" } else { os };
                        let label = format!(
                            "Downloading {}/{}@{}",
                            dep.organisation, dep.name, resolved_str
                        );
                        let binary = client
                            .download_component_binary(
                                &dep.organisation,
                                &dep.name,
                                &resolved_str,
                                registry_os,
                                arch,
                                Some(&label),
                            )
                            .await
                            .with_context(|| {
                                format!(
                                    "failed to download {}/{}@{} ({}/{})",
                                    dep.organisation, dep.name, resolved_str, os, arch
                                )
                            })?;
                        let size_bytes = binary.len();
                        lim.add_bytes(size_bytes as u64);

                        let (sha256, _cache_path) =
                            component_binary::store_binary_in_cache(&binary)?;
                        drop(binary);

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
                            message: Some(UpdateMessage::Success(format!(
                                "Updated {}/{}@{}",
                                dep.organisation, dep.name, resolved_str
                            ))),
                            updated: true,
                        })
                    } else {
                        // Files-based (CUE-only library or Deno component).
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
                                message: Some(UpdateMessage::Success(format!(
                                    "{}/{}@{} up to date",
                                    dep.organisation, dep.name, resolved_str
                                ))),
                                ..Default::default()
                            }),
                            EnsureCachedOutcome::Downloaded => Ok(DepUpdate {
                                message: Some(UpdateMessage::Success(format!(
                                    "Updated {}/{}@{} (files)",
                                    dep.organisation, dep.name, resolved_str
                                ))),
                                updated: true,
                                ..Default::default()
                            }),
                            EnsureCachedOutcome::BinaryRequiresPlatformDownload => {
                                // Manifest probe said non-binary but the
                                // ensure helper disagreed. Race or stale
                                // manifest cache — skip with a hint instead
                                // of erroring loudly.
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

        // Pass 3 (serial): replay output in dependency order and apply the
        // lockfile writes. Every dep got its chance to run before the first
        // error surfaces, so one unreachable component does not discard the
        // downloads that succeeded.
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
                message.emit();
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
