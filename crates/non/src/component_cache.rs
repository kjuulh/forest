use std::path::PathBuf;

use crate::{
    state::State,
    user_locations::{UserLocations, UserLocationsState},
};

pub struct ComponentCache {
    locations: UserLocations,
}

impl ComponentCache {
    pub async fn get_component_cache(&self) -> anyhow::Result<PathBuf> {
        let cache = self.locations.ensure_get_cache().await?;

        Ok(cache.join("components"))
    }
}

pub trait ComponentCacheState {
    fn component_cache(&self) -> ComponentCache;
}

impl ComponentCacheState for State {
    fn component_cache(&self) -> ComponentCache {
        ComponentCache {
            locations: self.user_locations(),
        }
    }
}
