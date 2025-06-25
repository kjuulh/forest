use std::collections::BTreeMap;

use anyhow::Context;
use serde::Deserialize;
use tokio::sync::OnceCell;

use crate::{
    services::project::ProjectDependency,
    state::State,
    user_locations::{UserLocations, UserLocationsState},
};

pub struct UserConfigService {
    locations: UserLocations,

    config: OnceCell<UserConfig>,
}

const USER_CONFIG_FILE: &str = "non.toml";

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
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct UserConfig {
    pub dependencies: BTreeMap<String, ProjectDependency>,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            dependencies: BTreeMap::default(),
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
            config: OnceCell::const_new(),
        }
    }
}
