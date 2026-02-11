use std::{fs::File, path::PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::state::State;

#[derive(Clone)]
pub struct UserStateLoader {
    path: PathBuf,
}

impl UserStateLoader {
    fn lock_path(&self) -> PathBuf {
        self.path.join("user-state.lock")
    }

    fn state_path(&self) -> PathBuf {
        self.path.join("user-state.json")
    }

    fn ensure_dir(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.state_path().parent() {
            std::fs::create_dir_all(parent).context("could not create state dir")?;
        }
        Ok(())
    }

    /// Acquire an exclusive file lock for cross-process coordination.
    /// The lock is released when the returned `File` is dropped.
    fn acquire_lock(&self) -> anyhow::Result<File> {
        self.ensure_dir()?;
        let file = File::create(self.lock_path()).context("could not create lock file")?;
        file.lock().context("could not acquire file lock")?;
        Ok(file)
    }

    pub async fn get_state(&self) -> anyhow::Result<Option<UserState>> {
        let state_path = self.state_path();

        if !state_path.exists() {
            return Ok(None);
        }

        let output = tokio::fs::read(&state_path).await?;

        let state: UserState =
            serde_json::from_slice(&output).context("could not form UserState from config")?;

        Ok(Some(state))
    }

    pub async fn set_state(&self, state: &UserState) -> anyhow::Result<()> {
        let state_path = self.state_path();

        self.ensure_dir()?;

        let _lock = self.acquire_lock()?;

        let output = serde_json::to_vec_pretty(state).context("could not serialize user state")?;

        let mut file = tokio::fs::File::create(state_path)
            .await
            .context("could not create user state file")?;

        file.write_all(&output).await?;
        file.flush().await?;

        Ok(())
    }

    /// Read the state while holding the file lock.
    /// Useful for check-then-act patterns (e.g. refresh token flow).
    pub async fn read_locked(&self) -> anyhow::Result<(Option<UserState>, File)> {
        self.ensure_dir()?;
        let lock = self.acquire_lock()?;

        let state_path = self.state_path();
        if !state_path.exists() {
            return Ok((None, lock));
        }

        let output = tokio::fs::read(&state_path).await?;
        let state: UserState =
            serde_json::from_slice(&output).context("could not form UserState from config")?;

        Ok((Some(state), lock))
    }

    /// Write the state while already holding the lock from `read_locked`.
    pub async fn write_locked(&self, state: &UserState, _lock: &File) -> anyhow::Result<()> {
        let state_path = self.state_path();

        let output = serde_json::to_vec_pretty(state).context("could not serialize user state")?;

        let mut file = tokio::fs::File::create(state_path)
            .await
            .context("could not create user state file")?;

        file.write_all(&output).await?;
        file.flush().await?;

        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserState {
    pub user_id: String,
    pub username: String,
    pub emails: Vec<String>,
    pub access_token: String,
    pub refresh_access: String,
    /// Unix timestamp at which the client should attempt a token refresh.
    /// Computed as the midpoint between token issuance and expiry.
    #[serde(default)]
    pub refresh_after: Option<i64>,
}

/// Compute the midpoint between now and the session expiry.
/// Tokens should be refreshed after this timestamp.
pub fn compute_refresh_after(now: i64, expires_at: i64) -> i64 {
    now + (expires_at - now) / 2
}

pub trait UserStateLoaderState {
    fn user_state(&self) -> UserStateLoader;
}

impl UserStateLoaderState for State {
    fn user_state(&self) -> UserStateLoader {
        let forest_dir = dirs::data_local_dir()
            .expect("to be able to get data dir")
            .join("forest");

        UserStateLoader { path: forest_dir }
    }
}
