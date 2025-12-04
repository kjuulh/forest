use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    str::FromStr,
    sync::Arc,
};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, sync::OnceCell};

use crate::{
    state::State,
    user_locations::{UserLocations, UserLocationsState},
};

#[derive(Clone)]
pub struct UserConfigService {
    locations: UserLocations,

    config: Arc<OnceCell<UserConfig>>,
}

const USER_CONFIG_FILE: &str = "forest.toml";

impl UserConfigService {
    pub async fn get_user_config(&self) -> anyhow::Result<&UserConfig> {
        let config = self
            .config
            .get_or_try_init(|| async {
                let config_path = self.locations.get_config().join(USER_CONFIG_FILE);

                if !config_path.exists() {
                    return Ok::<_, anyhow::Error>(UserConfig::default());
                }

                let file = tokio::fs::read_to_string(&config_path)
                    .await
                    .context(format!(
                        "failed to load config file at path: {}",
                        config_path.display()
                    ))?;

                let user_config: UserConfig =
                    toml::from_str(&file).context("failed to parse user config")?;

                Ok(user_config)
            })
            .await?;

        Ok(config)
    }

    pub async fn set(self, key: &str, value: &str) -> anyhow::Result<()> {
        let config_path = self.locations.get_config().join(USER_CONFIG_FILE);

        let mut user_config = if !config_path.exists() {
            toml_edit::DocumentMut::default()
        } else {
            let file = tokio::fs::read_to_string(&config_path)
                .await
                .context(format!(
                    "failed to load config file at path: {}",
                    config_path.display()
                ))?;

            toml_edit::DocumentMut::from_str(&file).context("failed to parse user config")?
        };

        if let Some(table) = user_config["user"].as_table() {
            user_config["user"] = toml_edit::Item::Table(table.clone());
        }

        if !user_config.contains_table("user") {
            user_config["user"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let table = user_config["user"].as_table_mut().unwrap();

        table[key] = toml_edit::value(value);

        if !config_path.exists()
            && let Some(parent) = config_path.parent()
        {
            tokio::fs::create_dir_all(&parent)
                .await
                .context("failed to create config dir")?;
        }

        let mut config = tokio::fs::File::create(config_path)
            .await
            .context("failed to create config file")?;

        let output = user_config.to_string();

        config
            .write_all(output.as_bytes())
            .await
            .context("failed to write to file")?;
        config.flush().await?;

        Ok(())
    }

    pub(crate) async fn add_dependency(
        &self,
        name: &str,
        namespace: &str,
        version: &str,
    ) -> anyhow::Result<()> {
        let config_path = self.locations.get_config().join(USER_CONFIG_FILE);

        let mut user_config = if !config_path.exists() {
            toml_edit::DocumentMut::default()
        } else {
            let file = tokio::fs::read_to_string(&config_path)
                .await
                .context(format!(
                    "failed to load config file at path: {}",
                    config_path.display()
                ))?;

            toml_edit::DocumentMut::from_str(&file).context("failed to parse user config")?
        };

        if !user_config.contains_table("dependencies") {
            user_config["dependencies"] = toml_edit::Item::Table(toml_edit::Table::new());
        }

        if let Some(table) = user_config["dependencies"].as_table() {
            user_config["dependencies"] = toml_edit::Item::Table(table.clone());
        }

        let table = user_config["dependencies"].as_table_mut().unwrap();

        if table.contains_key(name) && table.contains_key(&format!("{namespace}/{name}")) {
            anyhow::bail!("dependency already exists in user config");
        }

        let mut dependency_table = toml_edit::InlineTable::new();
        dependency_table.insert(
            "version",
            toml_edit::Value::String(toml_edit::Formatted::new(version.into())),
        );

        if namespace == "forest" {
            table[name] = toml_edit::value(toml_edit::Value::InlineTable(dependency_table));
        } else {
            table[&format!("{namespace}/{name}")] =
                toml_edit::value(toml_edit::Value::InlineTable(dependency_table));
        }

        if !config_path.exists()
            && let Some(parent) = config_path.parent()
        {
            tokio::fs::create_dir_all(&parent)
                .await
                .context("failed to create config dir")?;
        }

        let mut config = tokio::fs::File::create(config_path)
            .await
            .context("failed to create config file")?;

        let output = user_config.to_string();

        config
            .write_all(output.as_bytes())
            .await
            .context("failed to write to file")?;
        config.flush().await?;

        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct UserConfig {
    #[serde(default)]
    pub user: UserSection,

    #[serde(default)]
    pub dependencies: BTreeMap<String, GlobalDependency>,
}
impl UserConfig {
    fn update_user_data(&mut self, key: &str, value: &str) {
        self.user.insert(key.into(), value.into());
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct GlobalDependency {
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Default, Serialize)]
pub struct UserSection(BTreeMap<String, String>);

impl Deref for UserSection {
    type Target = BTreeMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for UserSection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[allow(clippy::derivable_impls)]
impl Default for UserConfig {
    fn default() -> Self {
        Self {
            dependencies: BTreeMap::default(),
            user: UserSection::default(),
        }
    }
}

pub trait UserConfigServiceState {
    fn user_config_service(&self) -> UserConfigService;
}

impl UserConfigServiceState for State {
    fn user_config_service(&self) -> UserConfigService {
        UserConfigService {
            locations: self.user_locations(),
            config: Arc::new(OnceCell::const_new()),
        }
    }
}
