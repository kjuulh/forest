use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use anyhow::Context;

use crate::state::State;

struct Locations {
    cache: PathBuf,
    config: PathBuf,
    data: PathBuf,
}

#[derive(Clone)]
pub struct UserLocations {
    locations: Arc<OnceLock<Locations>>,
}

impl UserLocations {
    pub fn get_cache(&self) -> PathBuf {
        self.get_locations().cache.to_path_buf()
    }

    pub async fn ensure_get_cache(&self) -> anyhow::Result<PathBuf> {
        let path = self.get_cache();

        tokio::fs::create_dir_all(&path).await.context(format!(
            "failed to create dir for cache: {}",
            &path.display()
        ))?;

        Ok(path)
    }

    pub fn get_config(&self) -> PathBuf {
        self.get_locations().config.to_path_buf()
    }

    pub async fn ensure_get_config(&self) -> anyhow::Result<PathBuf> {
        let path = self.get_config();

        tokio::fs::create_dir_all(&path).await.context(format!(
            "failed to create dir for config: {}",
            &path.display()
        ))?;

        Ok(path)
    }

    pub fn get_data(&self) -> PathBuf {
        self.get_locations().data.to_path_buf()
    }

    pub async fn ensure_get_data(&self) -> anyhow::Result<PathBuf> {
        let path = self.get_data();

        tokio::fs::create_dir_all(&path).await.context(format!(
            "failed to create dir for data: {}",
            &path.display()
        ))?;

        Ok(path)
    }

    #[tracing::instrument(skip(self), level = "trace")]
    fn get_locations(&self) -> &Locations {
        self.locations.get_or_init(|| {
            tracing::trace!("initializing user locations");

            let cache_dir = dirs::cache_dir()
                .expect("to be able to get cache dir")
                .join("non");
            let config_dir = dirs::config_dir()
                .expect("to be able to get config dir")
                .join("non");
            let data_dir = dirs::data_dir()
                .expect("to be able to get data dir")
                .join("non");

            Locations {
                cache: cache_dir,
                config: config_dir,
                data: data_dir,
            }
        })
    }
}

pub trait UserLocationsState {
    fn user_locations(&self) -> UserLocations;
}

impl UserLocationsState for State {
    fn user_locations(&self) -> UserLocations {
        UserLocations {
            locations: Arc::new(OnceLock::new()),
        }
    }
}
