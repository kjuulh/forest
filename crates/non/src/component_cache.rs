use std::path::PathBuf;

use crate::{
    services::component_parser::{ComponentParser, ComponentParserState, models::RawComponent},
    state::State,
    user_locations::{UserLocations, UserLocationsState},
};

pub mod models {
    #[derive(Clone, Debug, Default)]
    pub struct LocalComponents {
        pub components: Vec<LocalComponent>,
    }

    impl LocalComponents {
        pub fn find(&self, name: &str, namespace: &str, version: &str) -> Option<&LocalComponent> {
            self.components
                .iter()
                .find(|c| c.namespace == namespace && c.name == name && c.version == version)
        }

        pub fn diff_right(&self, right: Vec<impl Into<LocalComponent>>) -> LocalComponents {
            let components: Vec<LocalComponent> =
                right.into_iter().map(|c| c.into()).collect::<Vec<_>>();

            let right_components = components
                .iter()
                .filter(|r| self.components.iter().any(|l| l == *r))
                .cloned()
                .collect::<Vec<_>>();

            LocalComponents {
                components: right_components,
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct LocalComponent {
        pub name: String,
        pub namespace: String,
        pub version: String,
    }
}
use anyhow::Context;
use models::*;
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

    pub async fn get_local_components(&self) -> anyhow::Result<LocalComponents> {
        let component_cache_path = self.get_component_cache().await?;

        // cache is [namespace]/[name]/[version]/[component...]

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

        Ok(LocalComponents {
            components: components.into_iter().map(|i| i.into()).collect(),
        })
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

        if let Some(parent) = file_path.parent() {
            if !parent.exists() {
                tracing::trace!("creating component dir: {}", parent.display());

                tokio::fs::create_dir_all(parent)
                    .await
                    .context("failed to create path")?;
            }
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

impl From<RawComponent> for LocalComponent {
    fn from(value: RawComponent) -> Self {
        Self {
            name: value.component_spec.component.name,
            namespace: value.component_spec.component.namespace,
            version: value.component_spec.component.version,
        }
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
