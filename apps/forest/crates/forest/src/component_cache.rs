use std::path::{Path, PathBuf};

use crate::{
    component_cache::models::{CacheComponent, CacheComponents},
    services::component_parser::{ComponentParser, ComponentParserState},
    state::State,
    user_locations::{UserLocations, UserLocationsState},
};

pub mod models;

const NON_COMPONENT_DIRS: [&str; 2] = ["bin", "include"];

fn apply_cache_identity(
    component: &mut CacheComponent,
    organisation: &str,
    name: &str,
    version_dir: &str,
) {
    match semver::Version::parse(version_dir) {
        Ok(version) => {
            if version != component.version {
                tracing::debug!(
                    %organisation,
                    %name,
                    cached = %version,
                    manifest = %component.version,
                    "cached component manifest names another version; using the registry version",
                );
            }
            component.organisation = organisation.to_string();
            component.name = name.to_string();
            component.version = version;
        }
        Err(e) => {
            tracing::warn!(%organisation, %name, %version_dir, "cache directory is not a version: {e}");
        }
    }
    component.source = models::CacheComponentSource::Versioned(component.version.clone());
}

use anyhow::Context;

#[derive(Clone)]
pub struct ComponentCache {
    locations: UserLocations,
    component_parser: ComponentParser,
}

impl ComponentCache {
    pub async fn get_component_cache(&self) -> anyhow::Result<PathBuf> {
        let cache = self.locations.ensure_get_cache().await?;

        Ok(cache.join("components"))
    }

    pub async fn versioned_component_dir(
        &self,
        organisation: &str,
        name: &str,
        version: &str,
    ) -> anyhow::Result<PathBuf> {
        Ok(self
            .get_component_cache()
            .await?
            .join(organisation)
            .join(name)
            .join(version))
    }

    pub async fn get_local_components(&self) -> anyhow::Result<CacheComponents> {
        let component_cache_path = self
            .get_component_cache()
            .await
            .context("failed to get cache")?;

        if !component_cache_path.exists() {
            tracing::debug!("found no local cache, skipping");
            return Ok(CacheComponents::default());
        }

        let mut components = Vec::new();
        tracing::trace!("scanning component cache");

        let mut organisation_entries = tokio::fs::read_dir(component_cache_path)
            .await
            .context("read organisations")?;
        while let Some(organisation_entry) = organisation_entries
            .next_entry()
            .await
            .context("read organisations entry")?
        {
            if NON_COMPONENT_DIRS
                .contains(&organisation_entry.file_name().to_string_lossy().as_ref())
            {
                continue;
            }
            let mut name_entries = tokio::fs::read_dir(organisation_entry.path())
                .await
                .context(anyhow::anyhow!(
                    "read names for organisation: {}",
                    organisation_entry.path().to_string_lossy()
                ))?;

            while let Some(name_entry) =
                name_entries.next_entry().await.context(anyhow::anyhow!(
                    "read name entry for organisation: {}",
                    organisation_entry.path().to_string_lossy()
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
                    let mut component =
                        match self.get_component_from_path(&version_entry.path()).await {
                            Ok(component) => component,
                            Err(e) => {
                                tracing::debug!(
                                    path = %version_entry.path().to_string_lossy(),
                                    "skipping unparseable component in cache: {e:#}",
                                );
                                continue;
                            }
                        };

                    apply_cache_identity(
                        &mut component,
                        &organisation_entry.file_name().to_string_lossy(),
                        &name_entry.file_name().to_string_lossy(),
                        &version_entry.file_name().to_string_lossy(),
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
        tracing::trace!("getting component");

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
            .join(&component.organisation)
            .join(&component.name)
            .join(component.version.to_string());

        Ok(file_path)
    }

    pub async fn add_file(
        &self,
        name: &str,
        organisation: &str,
        version: &str,
        file_path: &str,
        file_content: &[u8],
    ) -> anyhow::Result<()> {
        if file_path.is_empty()
            || file_path.starts_with('/')
            || file_path.starts_with('\\')
            || file_path
                .split(['/', '\\'])
                .any(|seg| seg == ".." || seg.is_empty())
        {
            anyhow::bail!("refusing unsafe component file path: {file_path:?}");
        }

        let component_root = self
            .get_component_cache()
            .await?
            .join(organisation)
            .join(name)
            .join(version);
        let dest = component_root.join(file_path);

        if let Some(parent) = dest.parent()
            && !parent.exists()
        {
            tracing::trace!("creating component dir: {}", parent.display());
            tokio::fs::create_dir_all(parent)
                .await
                .context("failed to create path")?;
        }

        tracing::trace!("writing component file: {}", dest.display());
        tokio::fs::write(&dest, file_content)
            .await
            .with_context(|| format!("failed to write component file {}", dest.display()))?;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn cached(version: &str) -> CacheComponent {
        CacheComponent {
            name: "greeter".into(),
            organisation: "my-org".into(),
            version: semver::Version::parse(version).unwrap(),
            dependencies: vec![],
            requirements: vec![],
            commands: Default::default(),
            path: PathBuf::from("/cache/components/my-org/greeter/0.4.0-rc.1"),
            source: models::CacheComponentSource::Unknown,
        }
    }

    #[test]
    fn the_cache_directory_names_the_version_even_when_the_manifest_disagrees() {
        let mut component = cached("0.3.0");

        apply_cache_identity(&mut component, "my-org", "greeter", "0.4.0-rc.1");

        assert_eq!(component.version.to_string(), "0.4.0-rc.1");
        assert!(matches!(
            component.source,
            models::CacheComponentSource::Versioned(ref v) if v.to_string() == "0.4.0-rc.1"
        ));
    }

    #[test]
    fn a_directory_that_is_not_a_version_keeps_the_manifest_version() {
        let mut component = cached("0.3.0");

        apply_cache_identity(&mut component, "my-org", "greeter", "scratch");

        assert_eq!(component.version.to_string(), "0.3.0");
    }
}
