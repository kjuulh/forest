use std::path::{Path, PathBuf};

use crate::{
    component_cache::models::{CacheComponent, CacheComponents},
    services::component_parser::{ComponentParser, ComponentParserState, models::RawComponent},
    state::State,
    user_locations::{UserLocations, UserLocationsState},
};

pub mod models;

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

        let mut namespace_entries = tokio::fs::read_dir(component_cache_path)
            .await
            .context("read namespaces")?;
        while let Some(namespace_entry) = namespace_entries
            .next_entry()
            .await
            .context("read namespaces entry")?
        {
            let mut name_entries =
                tokio::fs::read_dir(namespace_entry.path())
                    .await
                    .context(anyhow::anyhow!(
                        "read names for namespace: {}",
                        namespace_entry.path().to_string_lossy()
                    ))?;

            while let Some(name_entry) =
                name_entries.next_entry().await.context(anyhow::anyhow!(
                    "read name entry for namespace: {}",
                    namespace_entry.path().to_string_lossy()
                ))?
            {
                let mut version_entries =
                    tokio::fs::read_dir(name_entry.path())
                        .await
                        .context(anyhow::anyhow!(
                            "read versions for name {}",
                            name_entry.path().to_string_lossy()
                        ))?;

                while let Some(version_entry) =
                    version_entries.next_entry().await.context(anyhow::anyhow!(
                        "read version entry for name {}",
                        name_entry.path().to_string_lossy()
                    ))?
                {
                    let mut component = self.get_component_from_path(&version_entry.path()).await?;

                    component.source = models::CacheComponentSource::Versioned(
                        component
                            .version
                            .parse()
                            .context("parsing semver for versioned component")?,
                    );

                    components.push(component);
                }
            }
        }
        tracing::trace!("done scanning component cache");

        Ok(CacheComponents(components))
    }
    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn get_component_from_path(&self, path: &Path) -> anyhow::Result<CacheComponent> {
        tracing::debug!("getting component");

        let component = self
            .component_parser
            .parse(path)
            .await
            .context(anyhow::anyhow!("parse file for {}", path.to_string_lossy()))?;

        component.try_into()
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
