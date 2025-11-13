use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::{
    component_cache::{
        ComponentCache, ComponentCacheState,
        models::{CacheComponent, CacheComponents},
    },
    grpc::{GrpcClient, GrpcClientState},
    models::{Dependencies, Dependency, DependencyType, Project},
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

pub struct ComponentsService {
    registry: ComponentRegistry,
    component_cache: ComponentCache,
    grpc: GrpcClient,
    parser: ComponentParser,
    deployment: ComponentDeploymentService,
    user_config: UserConfigService,
}

impl ComponentsService {
    pub async fn sync_components(
        &self,
        project: Option<Project>,
    ) -> anyhow::Result<CacheComponents> {
        tracing::debug!("syncing components");

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

        let (existing_deps, missing_deps) = local_components.diff(deps.dependencies.clone());
        for dep in existing_deps.dependencies {
            match dep.dependency_type {
                crate::models::DependencyType::Versioned(version) => {
                    tracing::debug!(
                        "local deps already exists: {}/{}@{}",
                        dep.namespace,
                        dep.name,
                        version
                    );
                }
                crate::models::DependencyType::Local(path) => {
                    tracing::debug!(
                        "local deps already exists: {}/{}#{}",
                        dep.namespace,
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
                    .get_component_version(&dep.name, &dep.namespace, &version.to_string())
                    .await?
                    .ok_or(anyhow::anyhow!("failed to find upstream component"))?;

                upstream.push(upstream_component);
            }
        }

        // Download deps
        for dep in upstream {
            self.download_component(&dep.id, &dep.name, &dep.namespace, &dep.version)
                .await?;
        }

        let mut local_deps = self
            .component_cache
            .get_local_components()
            .await
            .context("failed to get local components")?;

        for dependency in &deps.dependencies {
            match &dependency.dependency_type {
                DependencyType::Versioned(version) => continue,
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

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn get_component(
        &self,
        dep: &Dependency,
    ) -> anyhow::Result<UpstreamProjectDependency> {
        match &dep.dependency_type {
            DependencyType::Versioned(version) => {
                let component_version = self
                    .registry
                    .get_component_version(&dep.name, &dep.namespace, &version.to_string())
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
            DependencyType::Local(path) => todo!(),
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
        let user_config = self.user_config.get_user_config().await?;

        let deps: Dependencies = user_config.try_into()?;

        let local_deps = self
            .component_cache
            .get_local_components()
            .await
            .context("failed to get local components")?;

        // FIXME(kjuulh): implement inits
        // let local = local_deps
        //     .get(deps.dependencies)
        //     .context("failed to find all required local dependencies")?;

        // Ok(local.get_init())
        todo!()
    }

    async fn download_component(
        &self,
        id: &str,
        name: &str,
        namespace: &str,
        version: &str,
    ) -> anyhow::Result<()> {
        let mut stream = self.grpc.get_component_files(id).await?;

        while let Some(item) = stream.next().await.transpose()? {
            self.component_cache
                .add_file(
                    name,
                    namespace,
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
            namespace: value.namespace,
            version: value.version.parse()?,
        })
    }
}

impl TryFrom<CacheComponent> for Dependency {
    type Error = anyhow::Error;

    fn try_from(value: CacheComponent) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            namespace: value.namespace,
            dependency_type: DependencyType::Versioned(value.version.parse()?),
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
        ComponentsService {
            registry: self.component_registry(),
            component_cache: self.component_cache(),
            grpc: self.grpc_client(),
            parser: self.component_parser(),
            deployment: self.component_deployment_service(),
            user_config: self.user_config_service(),
        }
    }
}
