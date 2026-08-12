use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use crate::{
    component_cache::{
        ComponentCache, ComponentCacheState,
        models::{CacheComponent, CacheComponents},
    },
    forest_context::{ForestContext, ForestContextState},
    grpc::{GrpcClient, GrpcClientState},
    models::{
        ComponentReference, ComponentSource, Dependencies, Dependency, DependencyType, Project,
    },
    state::State,
    user_config::{UserConfigService, UserConfigServiceState},
};

use super::{
    component_deployment::{ComponentDeploymentService, ComponentDeploymentServiceState},
    component_parser::{ComponentParser, ComponentParserState, models::RawComponent},
    component_registry::{ComponentRegistry, ComponentRegistryState, models::RegistryComponent},
};

use anyhow::Context;
use futures::StreamExt;

pub mod models;
use models::*;
use tokio::sync::OnceCell;

#[derive(Clone)]
pub struct ComponentsService {
    registry: ComponentRegistry,
    component_cache: ComponentCache,
    grpc: GrpcClient,
    parser: ComponentParser,
    deployment: ComponentDeploymentService,
    user_config: UserConfigService,
    ctx: ForestContext,
    /// Ceiling on concurrent registry downloads (DATA-505). Resolved once from
    /// `--download-concurrency` / `FOREST_DOWNLOAD_CONCURRENCY`.
    max_in_flight: usize,

    components_project: Arc<OnceCell<CacheComponents>>,
    components_user_config: Arc<OnceCell<CacheComponents>>,
}

impl ComponentsService {
    pub async fn get_components_project(
        &self,
        project: Project,
    ) -> anyhow::Result<&CacheComponents> {
        self.components_project
            .get_or_try_init(|| async move {
                let c = self.sync_components(Some(project)).await?;

                Ok::<_, anyhow::Error>(c)
            })
            .await
    }

    pub async fn get_components_component(&self) -> anyhow::Result<&CacheComponents> {
        // FIXME: implement proper support for components
        self.get_components_user_config().await
    }

    pub async fn get_components_user_config(&self) -> anyhow::Result<&CacheComponents> {
        self.components_user_config
            .get_or_try_init(|| async move {
                let c = self.sync_components(None).await?;

                Ok::<_, anyhow::Error>(c)
            })
            .await
    }

    pub async fn get_local_component(
        &self,
        component_ref: &ComponentReference,
    ) -> anyhow::Result<CacheComponent> {
        match &component_ref.source {
            ComponentSource::Local(path) => {
                let comp = self.component_cache.get_component_from_path(path).await?;

                Ok(comp)
            }
            ComponentSource::Versioned(_version) => {
                let comp =
                    self.get_cache_component(component_ref)
                        .await?
                        .ok_or(anyhow::anyhow!(
                            "failed to find component: {}",
                            component_ref
                        ))?;

                Ok(comp.clone())
            }
        }
    }

    pub async fn get_cache_component(
        &self,
        component_ref: &ComponentReference,
    ) -> anyhow::Result<Option<&CacheComponent>> {
        let components = self.get_components_component().await?;

        for component in components.iter() {
            if &component.component_ref() == component_ref {
                return Ok(Some(component));
            }
        }

        Ok(None)
    }

    async fn sync_components(&self, project: Option<Project>) -> anyhow::Result<CacheComponents> {
        let inherited = self.ctx.inherited();

        tracing::trace!("syncing components");

        // 1. Construct local store of existing components
        let deps = if let Some(project) = project {
            let project = project.clone();
            project.dependencies
        } else {
            let user_config = self.user_config.get_user_config().await?;
            let deps: Dependencies = user_config.try_into()?;

            deps
        };

        let local_deps = self
            .component_cache
            .get_local_components()
            .await
            .context("failed to get local components")?;

        let local_components = Dependencies {
            dependencies: local_deps
                .iter()
                .map(|c| Dependency::try_from(c.clone()))
                .collect::<anyhow::Result<Vec<_>>>()
                .context("failed to get upstream dependencies")?,
        };

        if !inherited {
            let (existing_deps, missing_deps) = local_components.diff(deps.dependencies.clone());
            for dep in existing_deps.dependencies {
                match dep.dependency_type {
                    crate::models::DependencyType::Versioned(version) => {
                        tracing::debug!(
                            "local deps already exists: {}/{}@{}",
                            dep.organisation,
                            dep.name,
                            version
                        );
                    }
                    crate::models::DependencyType::Local(path) => {
                        tracing::debug!(
                            "local deps already exists: {}/{}#{}",
                            dep.organisation,
                            dep.name,
                            path.display().to_string()
                        );
                    }
                }
            }

            // 2. Resolve the missing upstream versions. One RPC per dep, and
            //    they are independent, so fan them out (DATA-505) — this used
            //    to be a serial round-trip per dependency before any bytes
            //    moved at all.
            let limiter = crate::download::Limiter::new(self.max_in_flight);
            let to_resolve: Vec<_> = missing_deps
                .dependencies
                .iter()
                .filter_map(|dep| match &dep.dependency_type {
                    DependencyType::Versioned(version) => Some((dep.clone(), version.to_string())),
                    DependencyType::Local(_) => None,
                })
                .collect();

            let resolved = crate::download::map_bounded(
                to_resolve,
                Arc::clone(&limiter),
                |(dep, version), _lim| async move {
                    tracing::debug!("fetching upstream dep");
                    self.registry
                        .get_component_version(&dep.name, &dep.organisation, &version)
                        .await?
                        .ok_or(anyhow::anyhow!("failed to find upstream component"))
                },
            )
            .await;
            let upstream = resolved.into_iter().collect::<anyhow::Result<Vec<_>>>()?;

            // 3. Download deps — component kind decides v1 (files) vs v2
            //    (binary). Bounded, adaptive concurrency: the binaries are the
            //    large ones and each has a serial prologue (server-side S3
            //    fetch, hashing, disk write) worth overlapping.
            //
            //    `forest.lock` is loaded once and shared read-only for
            //    verification; the writes are applied below, after the fan-out
            //    drains, because a read-modify-write per download would lose
            //    entries.
            let project_dir = std::env::current_dir()?;
            let lockfile = crate::lockfile::LockFile::load(&project_dir).await?;

            let outcomes =
                crate::download::map_bounded(upstream, Arc::clone(&limiter), |dep, lim| {
                    let lockfile = &lockfile;
                    async move {
                        // Try to get manifest — if it exists and kind=binary,
                        // download binary
                        let manifest: Result<String, _> = self
                            .grpc
                            .get_component_manifest(&dep.organisation, &dep.name, &dep.version)
                            .await;

                        let is_binary = manifest
                            .as_ref()
                            .ok()
                            .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
                            .and_then(|v| v.get("kind")?.as_str().map(|s| s == "binary"))
                            .unwrap_or(false);

                        if is_binary {
                            self.download_binary_component(
                                &dep.name,
                                &dep.organisation,
                                &dep.version,
                                manifest.as_deref().ok(),
                                lockfile,
                                &lim,
                            )
                            .await
                            .map(Some)
                        } else {
                            self.download_component(
                                &dep.id,
                                &dep.name,
                                &dep.organisation,
                                &dep.version,
                            )
                            .await
                            .map(|()| None)
                        }
                    }
                })
                .await;

            // Surface the first failure, but only after every sibling has had
            // its chance to finish — a flaky dep must not silently drop the
            // work the others already did.
            let entries = outcomes
                .into_iter()
                .collect::<anyhow::Result<Vec<_>>>()?
                .into_iter()
                .flatten();

            let mut lockfile = lockfile;
            let mut touched = false;
            for entry in entries {
                lockfile.insert(entry);
                touched = true;
            }
            if touched {
                lockfile.save(&project_dir).await?;
            }
        }

        let mut local_deps = self
            .component_cache
            .get_local_components()
            .await
            .context("failed to get local components")?;

        for dependency in &deps.dependencies {
            match &dependency.dependency_type {
                DependencyType::Versioned(_version) => continue,
                DependencyType::Local(path) => {
                    let mut component = self.component_cache.get_component_from_path(path).await?;

                    component.source = crate::component_cache::models::CacheComponentSource::Local(
                        component.path.clone(),
                    );

                    local_deps.push(component);
                }
            }
        }

        Ok(local_deps)
    }

    /// Download a v2 binary component from the registry and store in the content-addressable cache.
    ///
    /// Returns the lockfile entry to record rather than writing it: several of
    /// these run concurrently (DATA-505) and a read-modify-write of the shared
    /// `forest.lock` inside each one would lose updates. The caller verifies
    /// against `lockfile` (read-only, safe to share) and applies the returned
    /// entries serially once the fan-out has drained.
    async fn download_binary_component(
        &self,
        name: &str,
        organisation: &str,
        version: &str,
        manifest_json: Option<&str>,
        lockfile: &crate::lockfile::LockFile,
        limiter: &crate::download::Limiter,
    ) -> anyhow::Result<crate::lockfile::LockEntry> {
        let (os, arch) = crate::services::component_binary::current_platform();

        // The registry stores macOS binaries under the "darwin" os key — publish
        // translates macos→darwin on upload (see publish.rs, where the manifest
        // validator requires "darwin"). Match it here so the download key lines
        // up; otherwise a macOS consumer asks for "macos/arm64" and gets a
        // spurious "binary not found". DATA-312.
        let registry_os = if os == "macos" { "darwin" } else { os };

        tracing::debug!(
            "downloading binary component {organisation}/{name}@{version} ({registry_os}/{arch})"
        );

        let label = format!("Downloading {organisation}/{name}@{version}");
        let binary = self
            .grpc
            .download_component_binary(organisation, name, version, registry_os, arch, Some(&label))
            .await
            .context("download binary from registry")?;
        let size_bytes = binary.len();
        // Feed the adaptive ramp: bytes actually moved is what it climbs.
        // The driver signals completion; this only reports the bytes.
        limiter.add_bytes(size_bytes as u64);

        // Store in content-addressable cache
        let (sha256, cache_path) =
            crate::services::component_binary::store_binary_in_cache(&binary)
                .context("store binary in cache")?;
        drop(binary);

        let sha256_prefixed = format!("sha256:{sha256}");

        // Verify against the lock file the caller loaded. Read-only, so it is
        // safe to do here inside the concurrent region; the corresponding
        // *write* is the caller's job.
        lockfile.verify(organisation, name, version, os, arch, &sha256_prefixed)?;

        let lock_entry = crate::lockfile::LockEntry {
            organisation: organisation.to_string(),
            name: name.to_string(),
            version: version.to_string(),
            source: crate::lockfile::LockSource::Registry {
                os: os.to_string(),
                arch: arch.to_string(),
                sha256: sha256_prefixed,
            },
        };

        tracing::info!(
            "cached binary at {} (sha256={}, {} bytes)",
            cache_path.display(),
            &sha256[..12],
            size_bytes
        );

        // Write meta.json to the component cache directory so resolve_binary can find it
        let cache_component_dir = dirs::cache_dir()
            .context("cache dir")?
            .join("forest")
            .join("components")
            .join(organisation)
            .join(name)
            .join(version);
        tokio::fs::create_dir_all(&cache_component_dir).await?;

        let platform_key = format!("{os}_{arch}");
        let mut meta = serde_json::json!({
            "organisation": organisation,
            "name": name,
            "version": version,
            "platforms": {
                platform_key: {
                    "sha256": sha256,
                    "size": size_bytes,
                }
            }
        });

        // Include descriptor from manifest if available
        if let Some(manifest) = manifest_json {
            if let Ok(m) = serde_json::from_str::<serde_json::Value>(manifest) {
                if let Some(caps) = m.get("capabilities") {
                    meta["descriptor"] = serde_json::json!({
                        "protocol_version": m.get("protocol_version").and_then(|v| v.as_str()).unwrap_or("1.0"),
                        "methods": caps.get("methods").cloned().unwrap_or(serde_json::Value::Array(vec![])),
                    });
                }
            }
        }

        // Write meta.json in the .forest/component/ dir within the cache component path
        let meta_dir = cache_component_dir.join(".forest").join("component");
        tokio::fs::create_dir_all(&meta_dir).await?;
        tokio::fs::write(
            meta_dir.join("meta.json"),
            serde_json::to_string_pretty(&meta)?,
        )
        .await?;

        // Also write a minimal forest.component.cue marker so is_v2_component returns true
        let marker = cache_component_dir.join("forest.component.cue");
        if !marker.exists() {
            tokio::fs::write(&marker, format!("// {organisation}/{name}@{version}\n")).await?;
        }

        // Download every published file. Two destinations:
        //   1. The component cache dir (faithful mirror — used by
        //      `release prepare` to read templates/, schemas/, etc).
        //   2. For .cue files: also vendor into the project's
        //      `cue.mod/pkg/forest.sh/{org}/{name}@v{major}/` so
        //      `import "forest.sh/{org}/{name}@v0"` in consumer CUE
        //      resolves.
        if let Ok(Some(comp)) = self
            .grpc
            .get_component_version(name, organisation, version)
            .await
        {
            if let Ok(mut file_stream) = self.grpc.get_component_files(&comp.id).await {
                use futures::StreamExt;
                let mut cue_files: Vec<(String, Vec<u8>)> = Vec::new();

                while let Some(item) = file_stream.next().await {
                    match item {
                        Ok(f) => {
                            if let Err(e) = self
                                .component_cache
                                .add_file(
                                    name,
                                    organisation,
                                    version,
                                    &f.file_path,
                                    &f.file_content,
                                )
                                .await
                            {
                                tracing::warn!(
                                    file = %f.file_path,
                                    error = %e,
                                    "failed to write component file to cache",
                                );
                            }
                            if f.file_path.ends_with(".cue") {
                                cue_files.push((f.file_path, f.file_content));
                            }
                        }
                        Err(e) => {
                            tracing::warn!("failed to stream component files: {e}");
                            break;
                        }
                    }
                }

                if !cue_files.is_empty() {
                    // Vendor into cue.mod/pkg/forest.sh/{org}/{name}@v0/
                    let project_dir = std::env::current_dir()?;
                    let major_version = version.split('.').next().unwrap_or("0");
                    let vendor_dir = project_dir
                        .join("cue.mod")
                        .join("pkg")
                        .join("forest.sh")
                        .join(organisation)
                        .join(format!("{name}@v{major_version}"));

                    tokio::fs::create_dir_all(&vendor_dir).await?;

                    for (file_path, content) in &cue_files {
                        let dest = vendor_dir.join(file_path);
                        if let Some(parent) = dest.parent() {
                            tokio::fs::create_dir_all(parent).await?;
                        }
                        tokio::fs::write(&dest, content).await?;
                        tracing::info!("vendored {}", dest.display());
                    }
                }
            }
        }

        Ok(lock_entry)
    }

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn get_component(
        &self,
        dep: &Dependency,
    ) -> anyhow::Result<UpstreamProjectDependency> {
        match &dep.dependency_type {
            DependencyType::Versioned(version) => {
                let component_version = self
                    .registry
                    .get_component_version(&dep.name, &dep.organisation, &version.to_string())
                    .await
                    .context("failed to get component version")?;

                component_version
                    .map(|c| c.try_into())
                    .transpose()?
                    .ok_or(anyhow::anyhow!(
                        "failed to find upstream component: {:?}",
                        dep
                    ))
            }
            DependencyType::Local(_path) => {
                anyhow::bail!("local dependencies cannot be resolved as upstream components")
            }
        }
    }

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn list_components(&self) -> anyhow::Result<()> {
        tracing::debug!("listing components");

        let components = self.registry.get_components().await?;

        for component in components.items() {
            println!("component: {}", component.fqn())
        }

        Ok(())
    }

    pub async fn get_inits(&self) -> anyhow::Result<BTreeMap<String, (String, CacheComponent)>> {
        let _user_config = self.user_config.get_user_config().await?;

        // let deps: Dependencies = user_config.try_into()?;

        // let local_deps = self
        //     .component_cache
        //     .get_local_components()
        //     .await
        //     .context("failed to get local components")?;

        // FIXME(kjuulh): implement inits
        anyhow::bail!("component init templates are not yet supported")
    }

    async fn download_component(
        &self,
        id: &str,
        name: &str,
        organisation: &str,
        version: &str,
    ) -> anyhow::Result<()> {
        tracing::trace!(name, organisation, version, "downloading component");
        let mut stream = self.grpc.get_component_files(id).await?;

        while let Some(item) = stream.next().await.transpose()? {
            self.component_cache
                .add_file(
                    name,
                    organisation,
                    version,
                    &item.file_path,
                    &item.file_content,
                )
                .await?;
        }

        Ok(())
    }

    /// Make sure a versioned dep is materialized in
    /// `~/.cache/forest/components/<org>/<name>/<version>/`. Used by
    /// `forest update`, `forest generate` (when walking version deps),
    /// and the deployment/run resolvers — all three need the cache
    /// populated to operate, so this is the single ensure-cached
    /// shim shared across them.
    ///
    /// Returns true if a fresh download happened, false if the cache
    /// was already populated. Binary components are out of scope here:
    /// those have a parallel `download_binary_component` path that owns
    /// platform/sha256 bookkeeping. We detect "binary" via the
    /// component manifest's `kind` field and skip — the caller is
    /// expected to use the binary path in that case.
    pub async fn ensure_versioned_dep_cached(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
    ) -> anyhow::Result<EnsureCachedOutcome> {
        let cache_dir = dirs::cache_dir()
            .context("locate cache dir")?
            .join("forest")
            .join("components")
            .join(organisation)
            .join(name)
            .join(version);

        // Already materialized? Presence of forest.component.cue
        // (always uploaded for v2 components) is our cheap probe.
        if cache_dir.join("forest.component.cue").exists() {
            return Ok(EnsureCachedOutcome::AlreadyCached);
        }

        // Determine kind. Binary deps go through the dedicated path.
        let manifest_raw = self
            .grpc
            .get_component_manifest(organisation, name, version)
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
            return Ok(EnsureCachedOutcome::BinaryRequiresPlatformDownload);
        }

        // Files-based (kind=cue / kind=deno / kind=files). Look up the
        // component id, then stream every uploaded file into the cache.
        // `download_component` already writes via `ComponentCache::add_file`
        // which puts files at `<cache_dir>/<file_path>` — matching exactly
        // what the publisher uploaded.
        let comp = self
            .grpc
            .get_component_version(name, organisation, version)
            .await
            .context("query component version metadata")?
            .ok_or_else(|| {
                anyhow::anyhow!("component {organisation}/{name}@{version} not found in registry")
            })?;

        self.download_component(&comp.id.to_string(), name, organisation, version)
            .await
            .with_context(|| format!("download files for {organisation}/{name}@{version}"))?;

        Ok(EnsureCachedOutcome::Downloaded)
    }
}

/// Result of [`ComponentsService::ensure_versioned_dep_cached`].
///
/// Callers interpret these as: `AlreadyCached` → no work, `Downloaded` →
/// we just materialized this version (good time to log a friendly nudge),
/// `BinaryRequiresPlatformDownload` → caller must invoke the per-platform
/// binary downloader instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsureCachedOutcome {
    AlreadyCached,
    Downloaded,
    BinaryRequiresPlatformDownload,
}

impl ComponentsService {
    // (continuation of the impl above — split for readability of the new
    // ensure_versioned_dep_cached helper, since the original impl block
    // contains a lot of unrelated methods.)

    pub async fn get_component_path(&self, component: &CacheComponent) -> anyhow::Result<PathBuf> {
        let path = self.component_cache.get_component_path(component).await?;

        Ok(path)
    }

    pub async fn get_staging_component(&self, path: &Path) -> anyhow::Result<RawComponent> {
        let component_spec = self.parser.parse(path).await?;

        Ok(component_spec)
    }

    pub async fn deploy_component(&self, raw_component: RawComponent) -> anyhow::Result<()> {
        self.deployment.deploy_component(raw_component).await?;

        Ok(())
    }
}

impl TryFrom<RegistryComponent> for UpstreamProjectDependency {
    type Error = anyhow::Error;

    fn try_from(value: RegistryComponent) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.parse()?,
            name: value.name,
            organisation: value.organisation,
            version: value.version.parse()?,
        })
    }
}

impl TryFrom<CacheComponent> for Dependency {
    type Error = anyhow::Error;

    fn try_from(value: CacheComponent) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            organisation: value.organisation,
            dependency_type: DependencyType::Versioned(value.version.to_string()),
        })
    }
}

impl ComponentsService {
    pub async fn get_components(&self) -> anyhow::Result<Components> {
        Ok(Components::default())
    }
}

pub trait ComponentsServiceState {
    fn components_service(&self) -> ComponentsService;
}

impl ComponentsServiceState for State {
    fn components_service(&self) -> ComponentsService {
        static ONCE: OnceLock<ComponentsService> = OnceLock::new();

        ONCE.get_or_init(|| ComponentsService {
            registry: self.component_registry(),
            component_cache: self.component_cache(),
            grpc: self.grpc_client(),
            parser: self.component_parser(),
            deployment: self.component_deployment_service(),
            user_config: self.user_config_service(),
            ctx: self.context(),
            max_in_flight: self.config.max_downloads_in_flight(),
            components_project: Arc::new(OnceCell::new()),
            components_user_config: Arc::new(OnceCell::new()),
        })
        .clone()
    }
}
