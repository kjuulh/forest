use std::{
    env::temp_dir,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use anyhow::Context;
use drop_queue::DropQueue;
use rand::Rng;

use crate::state::State;

#[derive(Clone)]
pub struct TempDirectories {
    drop_queue: DropQueue,
}

impl TempDirectories {
    pub async fn create_emphemeral_temp(&self) -> anyhow::Result<GuardedTempDirectory> {
        let temp = self.create_temp().await?;

        Ok(GuardedTempDirectory::new(self.drop_queue.clone(), temp))
    }

    pub fn inherit_temp(&self, path: &Path) -> TempDirectory {
        TempDirectory::Inherited {
            path: path.to_path_buf(),
        }
    }

    pub async fn create_temp(&self) -> anyhow::Result<TempDirectory> {
        self.garbage_collect().await.context("garbage collect")?;

        let random_path = self.base_dir().join(generate_id(10));

        tokio::fs::create_dir_all(&random_path)
            .await
            .context("create temp parent dir")?;

        Ok(TempDirectory::Owned {
            last_modified: SystemTime::now(),
            path: random_path,
        })
    }

    pub async fn index(&self) -> anyhow::Result<Index> {
        let base = self.base_dir();

        if !base.exists() {
            return Ok(Index::default());
        }

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
pub enum TempDirectory {
    Owned {
        last_modified: SystemTime,
        path: PathBuf,
    },
    Inherited {
        path: PathBuf,
    },
}

impl Deref for TempDirectory {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        match self {
            TempDirectory::Owned { path, .. } => path,
            TempDirectory::Inherited { path } => path,
        }
    }
}

impl TempDirectory {
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        self.path().display().to_string()
    }

    pub fn is_expired(&self) -> bool {
        match self {
            TempDirectory::Owned { last_modified, .. } => {
                let modified = last_modified
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
            TempDirectory::Inherited { .. } => false,
        }
    }

    async fn remove(&self) -> anyhow::Result<()> {
        tokio::fs::remove_dir_all(self.path()).await?;

        Ok(())
    }

    fn path(&self) -> &Path {
        match self {
            TempDirectory::Owned { path, .. } => path,
            TempDirectory::Inherited { path } => path,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Index {
    directories: Vec<TempDirectory>,
}

impl Index {
    fn add(&mut self, path: PathBuf, modified: SystemTime) {
        self.directories.push(TempDirectory::Owned {
            path,
            last_modified: modified,
        })
    }

    fn size_expired(&self) -> usize {
        self.directories.iter().filter(|d| d.is_expired()).count()
    }
}

pub struct GuardedTempDirectory {
    inner: Arc<InnerGuardedTempDirectory>,
}
impl GuardedTempDirectory {
    fn new(drop_queue: DropQueue, temp: TempDirectory) -> Self {
        Self {
            inner: Arc::new(InnerGuardedTempDirectory { drop_queue, temp }),
        }
    }
}
impl Deref for GuardedTempDirectory {
    type Target = PathBuf;
    fn deref(&self) -> &Self::Target {
        match &self.inner.temp {
            TempDirectory::Owned { path, .. } => path,
            TempDirectory::Inherited { path } => path,
        }
    }
}

struct InnerGuardedTempDirectory {
    drop_queue: DropQueue,
    temp: TempDirectory,
}

impl Drop for InnerGuardedTempDirectory {
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
