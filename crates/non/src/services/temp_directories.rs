use std::{env::temp_dir, path::PathBuf, time::SystemTime};

use rand::Rng;

use crate::{drop_queue::DropQueue, state::State};

pub struct TempDirectories {
    drop_queue: DropQueue,
}

impl TempDirectories {
    pub async fn create_emphemeral_temp(&self) -> anyhow::Result<GuardedTempDirectory> {
        let temp = self.create_temp().await?;

        Ok(GuardedTempDirectory {
            drop_queue: self.drop_queue.clone(),
            temp,
        })
    }

    pub async fn create_temp(&self) -> anyhow::Result<TempDirectory> {
        self.garbage_collect().await?;

        let random_path = self.base_dir().join(generate_id(10));

        tokio::fs::create_dir_all(&random_path).await?;

        Ok(TempDirectory {
            last_modified: SystemTime::now(),
            path: random_path,
        })
    }

    pub async fn index(&self) -> anyhow::Result<Index> {
        let base = self.base_dir();

        let mut directories = Index::default();

        let mut entries = tokio::fs::read_dir(base).await?;
        while let Some(entry) = entries.next_entry().await? {
            let metadata = entry.metadata().await?;
            let path = entry.path();

            directories.add(path, metadata.modified()?);
        }

        Ok(directories)
    }

    pub async fn garbage_collect(&self) -> anyhow::Result<()> {
        let index = self.index().await?;

        if index.size_expired() == 0 {
            return Ok(());
        }

        tracing::info!(
            garbage_count = index.size_expired(),
            "collecting tempdirs for deletion"
        );

        let workers = noworkers::Workers::new();
        for directory in index.directories.iter().filter(|d| d.is_expired()) {
            let directory = directory.clone();
            workers
                .add(move |_| async move {
                    tracing::debug!("removing temp dir: {}", directory.to_string());
                    directory.remove().await?;

                    Ok::<(), anyhow::Error>(())
                })
                .await?;
        }

        workers.wait().await?;

        Ok(())
    }

    fn base_dir(&self) -> PathBuf {
        temp_dir().join("non").join("tmp")
    }
}

pub trait TempDirectoriesState {
    fn temp_directories(&self) -> TempDirectories;
}

impl TempDirectoriesState for State {
    fn temp_directories(&self) -> TempDirectories {
        TempDirectories {
            drop_queue: self.drop_queue.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TempDirectory {
    last_modified: SystemTime,
    path: PathBuf,
}

impl TempDirectory {
    pub fn to_path_buf(&self) -> PathBuf {
        self.path.clone()
    }

    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        self.path.display().to_string()
    }

    pub fn is_expired(&self) -> bool {
        let modified = self
            .last_modified
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        const SECONDS_7_DAYS: u64 = 60 * 60 * 24 * 7;

        modified < now - SECONDS_7_DAYS
    }

    async fn remove(&self) -> anyhow::Result<()> {
        tokio::fs::remove_dir_all(&self.path).await?;

        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Index {
    directories: Vec<TempDirectory>,
}

impl Index {
    fn add(&mut self, path: PathBuf, modified: SystemTime) {
        self.directories.push(TempDirectory {
            path,
            last_modified: modified,
        })
    }

    fn size(&self) -> usize {
        self.directories.len()
    }

    fn size_expired(&self) -> usize {
        self.directories.iter().filter(|d| d.is_expired()).count()
    }
}

pub struct GuardedTempDirectory {
    drop_queue: DropQueue,
    temp: TempDirectory,
}

impl Drop for GuardedTempDirectory {
    fn drop(&mut self) {
        let temp = self.temp.clone();
        self.drop_queue
            .assign(move || async move { temp.remove().await })
            .expect("to be able to put item on the drop queue");
    }
}

const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789-_";

fn generate_id(len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| {
            let idx = rng.random_range(0..ALPHABET.len());
            ALPHABET[idx] as char
        })
        .collect()
}
