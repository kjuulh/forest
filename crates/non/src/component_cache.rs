use std::path::PathBuf;

use crate::{
    component_cache::models::{CacheComponent, CacheComponents},
    services::component_parser::{ComponentParser, ComponentParserState},
    state::State,
    user_locations::{UserLocations, UserLocationsState},
};

pub mod models {
    use std::ops::Deref;

    use anyhow::Context;

    use crate::services::component_parser::models::{
        RawComponent, RawComponentDependency, RawComponentRequirement, RawComponentRequirementType,
    };

    #[derive(Default)]
    pub struct CacheComponents(pub Vec<CacheComponent>);
    impl Deref for CacheComponents {
        type Target = Vec<CacheComponent>;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    #[derive(Debug, Clone)]
    pub struct CacheComponent {
        pub name: String,
        pub namespace: String,
        pub version: String,

        pub dependencies: Vec<CacheComponentDependency>,

        pub requirements: Vec<CacheComponentRequirement>,
    }

    impl TryFrom<RawComponent> for CacheComponent {
        type Error = anyhow::Error;

        fn try_from(value: RawComponent) -> Result<Self, Self::Error> {
            Ok(Self {
                name: value.component_spec.component.name,
                namespace: value.component_spec.component.namespace,
                version: value.component_spec.component.version,
                dependencies: value
                    .component_spec
                    .dependencies
                    .into_iter()
                    .map(|i| i.try_into())
                    .collect::<anyhow::Result<Vec<_>>>()?,
                requirements: value
                    .component_spec
                    .requirements
                    .into_iter()
                    .map(|i| i.try_into())
                    .collect::<anyhow::Result<Vec<_>>>()?,
            })
        }
    }

    #[derive(Debug, Clone)]
    pub struct CacheComponentDependency {
        pub name: String,
        pub namespace: String,
        pub version: semver::Version,
    }

    impl TryFrom<(String, RawComponentDependency)> for CacheComponentDependency {
        type Error = anyhow::Error;

        fn try_from(
            (name, dependency): (String, RawComponentDependency),
        ) -> Result<Self, Self::Error> {
            let (namespace, name) = match name.split_once("/") {
                Some((namespace, dep)) => (namespace, dep),
                None => ("non", name.as_str()),
            };

            let version = match dependency {
                RawComponentDependency::String(version) => version,
                RawComponentDependency::Detailed(dep) => dep.version,
            };

            let version =
                semver::Version::parse(&version).context("failed to parse dependency version")?;

            Ok(Self {
                name: name.into(),
                namespace: namespace.into(),
                version,
            })
        }
    }

    #[derive(Debug, Clone)]
    pub struct CacheComponentRequirement {
        pub name: String,
        pub description: Option<String>,
        pub default: Option<String>,
        pub r#type: Option<CacheComponentRequirementType>,
    }

    #[derive(Debug, Clone)]
    pub enum CacheComponentRequirementType {
        String,
    }

    impl TryFrom<(String, RawComponentRequirement)> for CacheComponentRequirement {
        type Error = anyhow::Error;

        fn try_from((entry, req): (String, RawComponentRequirement)) -> Result<Self, Self::Error> {
            Ok(Self {
                name: entry,
                description: req.description,
                default: req.default,
                r#type: req.r#type.map(|i| i.try_into()).transpose()?,
            })
        }
    }

    impl TryFrom<RawComponentRequirementType> for CacheComponentRequirementType {
        type Error = anyhow::Error;

        fn try_from(value: RawComponentRequirementType) -> Result<Self, Self::Error> {
            let val = match value {
                RawComponentRequirementType::String => Self::String,
            };

            Ok(val)
        }
    }
}

use anyhow::Context;
use tokio::io::AsyncWriteExt;

pub struct ComponentCache {
    locations: UserLocations,
    component_parser: ComponentParser,
}

impl ComponentCache {
    pub async fn get_component_cache(&self) -> anyhow::Result<PathBuf> {
        let cache = self.locations.ensure_get_cache().await?;

        Ok(cache.join("components"))
    }

    pub async fn get_local_components(&self) -> anyhow::Result<CacheComponents> {
        let component_cache_path = self
            .get_component_cache()
            .await
            .context("failed to get cache")?;

        // cache is [namespace]/[name]/[version]/[component...]
        if !component_cache_path.exists() {
            tracing::debug!("found no local cache, skipping");
            return Ok(CacheComponents::default());
        }

        let mut components = Vec::new();
        tracing::trace!("scanning component cache");

        let mut namespace_entries = tokio::fs::read_dir(component_cache_path).await?;
        while let Some(namespace_entry) = namespace_entries.next_entry().await? {
            let mut name_entries = tokio::fs::read_dir(namespace_entry.path()).await?;

            while let Some(name_entry) = name_entries.next_entry().await? {
                let mut version_entries = tokio::fs::read_dir(name_entry.path()).await?;

                while let Some(version_entry) = version_entries.next_entry().await? {
                    let component = self.component_parser.parse(&version_entry.path()).await?;

                    components.push(component);
                }
            }
        }
        tracing::trace!("done scanning component cache");

        Ok(CacheComponents(
            components
                .into_iter()
                .map(|i| i.try_into())
                .collect::<anyhow::Result<Vec<_>>>()?,
        ))
    }

    pub async fn get_component_path(&self, component: &CacheComponent) -> anyhow::Result<PathBuf> {
        let file_path = self
            .get_component_cache()
            .await?
            .join(&component.namespace)
            .join(&component.name)
            .join(&component.version);

        Ok(file_path)
    }

    pub async fn add_file(
        &self,
        name: &str,
        namespace: &str,
        version: &str,
        file_path: &str,
        file_content: &[u8],
    ) -> anyhow::Result<()> {
        let file_path = self
            .get_component_cache()
            .await?
            .join(namespace)
            .join(name)
            .join(version)
            .join(file_path);

        if let Some(parent) = file_path.parent()
            && !parent.exists()
        {
            tracing::trace!("creating component dir: {}", parent.display());

            tokio::fs::create_dir_all(parent)
                .await
                .context("failed to create path")?;
        }

        tracing::trace!("creating component file: {}", file_path.display());
        let mut file = tokio::fs::File::create_new(file_path)
            .await
            .context("failed to create component file")?;
        file.write_all(file_content)
            .await
            .context("failed to write to component file")?;
        file.flush()
            .await
            .context("failed to flush component file")?;

        Ok(())
    }
}

pub trait ComponentCacheState {
    fn component_cache(&self) -> ComponentCache;
}

impl ComponentCacheState for State {
    fn component_cache(&self) -> ComponentCache {
        ComponentCache {
            locations: self.user_locations(),
            component_parser: self.component_parser(),
        }
    }
}
