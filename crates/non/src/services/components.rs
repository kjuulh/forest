use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::{
    component_cache::{ComponentCache, ComponentCacheState, models::LocalComponent},
    grpc::{GrpcClient, GrpcClientState},
    state::State,
    user_config::{UserConfigService, UserConfigServiceState},
};

use super::{
    component_deployment::{ComponentDeploymentService, ComponentDeploymentServiceState},
    component_parser::{ComponentParser, ComponentParserState, models::RawComponent},
    component_registry::{ComponentRegistry, ComponentRegistryState, models::RegistryComponent},
    project::{ProjectParser, ProjectParserState},
};

pub mod models;
use anyhow::Context;
use futures::StreamExt;
use models::*;

pub struct ComponentsService {
    registry: ComponentRegistry,
    component_cache: ComponentCache,
    project_parser: ProjectParser,
    grpc: GrpcClient,
    parser: ComponentParser,
    deployment: ComponentDeploymentService,
    user_config: UserConfigService,
}

impl ComponentsService {
    pub async fn sync_components(&self) -> anyhow::Result<()> {
        let user_config = self.user_config.get_user_config().await?;

        // 1. Construct local store of existing components
        // let project = self
        //     .project_parser
        //     .get_project()
        //     .await
        //     .context("failed to get project")?;

        //let deps: Dependencies = project.try_into()?;
        let deps: Dependencies = user_config.try_into()?;

        let local_deps = self
            .component_cache
            .get_local_components()
            .await
            .context("failed to get local components")?;

        let local_components = Dependencies {
            dependencies: local_deps
                .components
                .iter()
                .map(|c| Dependency::try_from(c.clone()))
                .collect::<anyhow::Result<Vec<_>>>()
                .context("failed to get upstream dependencies")?,
        };

        let (existing_deps, missing_deps) = local_components.diff(deps.dependencies);
        for dep in existing_deps.dependencies {
            tracing::debug!(
                "local deps already exists: {}/{}@{}",
                dep.namespace,
                dep.name,
                dep.version
            );
        }

        // 2. Fetch upstream version that is missing
        let mut upstream = Vec::new();
        for dep in &missing_deps.dependencies {
            tracing::debug!("fetching upstream dep");
            let upstream_component = self
                .registry
                .get_component_version(&dep.name, &dep.namespace, &dep.version.to_string())
                .await?
                .ok_or(anyhow::anyhow!("failed to find upstream component"))?;

            upstream.push(upstream_component);
        }

        // Download deps

        for dep in upstream {
            self.download_component(&dep.id, &dep.name, &dep.namespace, &dep.version)
                .await?;
        }

        Ok(())
    }

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn get_component(
        &self,
        dep: &Dependency,
    ) -> anyhow::Result<UpstreamProjectDependency> {
        let component_version = self
            .registry
            .get_component_version(&dep.name, &dep.namespace, &dep.version.to_string())
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

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn list_components(&self) -> anyhow::Result<()> {
        tracing::debug!("listing components");

        let components = self.registry.get_components().await?;

        for component in components.items() {
            println!("component: {}", component.fqn())
        }

        Ok(())
    }

    pub async fn get_inits(&self) -> anyhow::Result<BTreeMap<String, (String, LocalComponent)>> {
        let user_config = self.user_config.get_user_config().await?;

        let deps: Dependencies = user_config.try_into()?;

        let local_deps = self
            .component_cache
            .get_local_components()
            .await
            .context("failed to get local components")?;

        let local = local_deps
            .get(deps.dependencies)
            .context("failed to find all required local dependencies")?;

        Ok(local.get_init())
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

    pub async fn get_component_path(&self, component: &LocalComponent) -> anyhow::Result<PathBuf> {
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

impl TryFrom<LocalComponent> for Dependency {
    type Error = anyhow::Error;

    fn try_from(value: LocalComponent) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            namespace: value.namespace,
            version: value.version.parse()?,
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
            project_parser: self.project_parser(),
            grpc: self.grpc_client(),
            parser: self.component_parser(),
            deployment: self.component_deployment_service(),
            user_config: self.user_config_service(),
        }
    }
}
