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

            // 2. Fetch upstream version that is missing
            let mut upstream = Vec::new();
            for dep in &missing_deps.dependencies {
                if let DependencyType::Versioned(version) = &dep.dependency_type {
                    tracing::debug!("fetching upstream dep");
                    let upstream_component = self
                        .registry
                        .get_component_version(&dep.name, &dep.organisation, &version.to_string())
                        .await?
                        .ok_or(anyhow::anyhow!("failed to find upstream component"))?;

                    upstream.push(upstream_component);
                }
            }

            // Download deps — check component kind to decide v1 (files) vs v2 (binary)
            for dep in upstream {
                // Try to get manifest — if it exists and kind=binary, download binary
                let manifest: Result<String, _> = self.grpc.get_component_manifest(
                    &dep.organisation,
                    &dep.name,
                    &dep.version,
                ).await;

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
                    )
                    .await?;
                } else {
                    self.download_component(&dep.id, &dep.name, &dep.organisation, &dep.version)
                        .await?;
                }
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
    async fn download_binary_component(
        &self,
        name: &str,
        organisation: &str,
        version: &str,
        manifest_json: Option<&str>,
    ) -> anyhow::Result<()> {
        let (os, arch) = crate::services::component_binary::current_platform();

        tracing::info!(
            "downloading binary component {organisation}/{name}@{version} ({os}/{arch})"
        );

        let binary = self
            .grpc
            .download_component_binary(organisation, name, version, os, arch)
            .await
            .context("download binary from registry")?;

        // Store in content-addressable cache
        let (sha256, cache_path) = crate::services::component_binary::store_binary_in_cache(&binary)
            .context("store binary in cache")?;

        let sha256_prefixed = format!("sha256:{sha256}");

        // Verify against lock file if present
        let project_dir = std::env::current_dir()?;
        let lockfile = crate::lockfile::LockFile::load(&project_dir).await?;
        lockfile.verify(organisation, name, version, os, arch, &sha256_prefixed)?;

        // Update lock file
        let mut lockfile = lockfile;
        lockfile.insert(crate::lockfile::LockEntry {
            organisation: organisation.to_string(),
            name: name.to_string(),
            version: version.to_string(),
            source: crate::lockfile::LockSource::Registry {
                os: os.to_string(),
                arch: arch.to_string(),
                sha256: sha256_prefixed,
            },
        });
        lockfile.save(&project_dir).await?;

        tracing::info!(
            "cached binary at {} (sha256={}, {} bytes)",
            cache_path.display(),
            &sha256[..12],
            binary.len()
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
                    "size": binary.len(),
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

        // Download CUE spec files and vendor into project's cue.mod/pkg/
        // This enables `import "forest.sh/{org}/{name}@v0"` in consumer CUE files
        if let Ok(Some(comp)) = self.grpc.get_component_version(name, organisation, version).await {
            if let Ok(mut file_stream) = self.grpc.get_component_files(&comp.id).await {
                use futures::StreamExt;
                let mut cue_files: Vec<(String, Vec<u8>)> = Vec::new();

                while let Some(item) = file_stream.next().await {
                    match item {
                        Ok(f) => {
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

        Ok(())
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
            DependencyType::Local(_path) => anyhow::bail!("local dependencies cannot be resolved as upstream components"),
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
            components_project: Arc::new(OnceCell::new()),
            components_user_config: Arc::new(OnceCell::new()),
        })
        .clone()
    }
}
