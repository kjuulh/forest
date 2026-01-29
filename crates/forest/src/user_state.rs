use std::path::PathBuf;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::state::State;

pub struct UserStateLoader {
    path: PathBuf,
}

impl UserStateLoader {
    pub async fn get_state(&self) -> anyhow::Result<Option<UserState>> {
        let user_path = self.path.join("user-state.json");

        if !user_path.exists() {
            return Ok(None);
        }

        let output = tokio::fs::read(&user_path).await?;

        let state: UserState =
            serde_json::from_slice(&output).context("could not form UserState from config")?;

        Ok(Some(state))
    }

    pub async fn set_state(&self, state: &UserState) -> anyhow::Result<()> {
        let user_path = self.path.join("user-state.json");

        if let Some(parent) = user_path.parent() {
            tokio::fs::create_dir_all(&parent)
                .await
                .context("could not create state dir")?;
        }

        let output = serde_json::to_vec_pretty(state).context("could not serialize user state")?;

        let mut file = tokio::fs::File::create(user_path)
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
    pub token: String,
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
