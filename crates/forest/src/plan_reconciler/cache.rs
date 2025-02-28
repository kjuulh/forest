use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

#[derive(Serialize, Deserialize, Clone, Debug)]
struct CacheFile {
    last_update: u64,
}

pub struct Cache {
    path: PathBuf,
}

impl Cache {
    pub fn new(destination: &Path) -> Self {
        Self {
            path: destination.join(".forest").join("plan.cache.json"),
        }
    }

    pub async fn is_cache_valid(&self) -> anyhow::Result<Option<u64>> {
        if !self.path.exists() {
            return Ok(None);
        }

        if let Ok(cache_config) = std::env::var("FOREST_CACHE").map(|c| c.trim().to_lowercase()) {
            if cache_config.eq("no") || cache_config.eq("false") || cache_config.eq("0") {
                return Ok(None);
            }
        }

        let file = tokio::fs::read_to_string(&self.path).await?;
        let cache_file: CacheFile = serde_json::from_str(&file)?;
        let unix_cache = std::time::Duration::from_secs(cache_file.last_update);
        let now = std::time::SystemTime::now().duration_since(UNIX_EPOCH)?;

        let cache_expire = now
            .as_secs()
            .saturating_sub(std::time::Duration::from_secs(60 * 60 * 8).as_secs());

        if unix_cache.as_secs() > cache_expire {
            return Ok(Some(unix_cache.as_secs().saturating_sub(cache_expire)));
        }

        Ok(None)
    }

    pub async fn upsert_cache(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let unix = std::time::SystemTime::now().duration_since(UNIX_EPOCH)?;
        let cache_file = CacheFile {
            last_update: unix.as_secs(),
        };
        let val = serde_json::to_string_pretty(&cache_file)?;

        let mut file = tokio::fs::File::create(&self.path).await?;
        file.write_all(val.as_bytes()).await?;

        Ok(())
    }
}
