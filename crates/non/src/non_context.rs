use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use anyhow::Context;
use tokio::sync::OnceCell;

use crate::{
    models::ComponentReference,
    services::temp_directories::{TempDirectories, TempDirectoriesState, TempDirectory},
    state::State,
};

#[derive(Clone)]
pub struct NonContext {
    parents: Vec<String>,
    tmp_dir: Arc<OnceCell<TempDirectory>>,
    component: Option<ComponentReference>,
    tmp: TempDirectories,
    context_id: String,
}

impl NonContext {
    pub fn from_env(tmp: TempDirectories) -> Self {
        let context_raw = std::env::var(Self::get_context_key()).unwrap_or_default();
        let parents = context_raw
            .split(',')
            .map(|s| s.to_string())
            .filter(|i| i.parse::<uuid::Uuid>().is_ok())
            .collect::<Vec<_>>();

        let tmp_dir = std::env::var(Self::get_tmp_key())
            .ok()
            .map(|t| tmp.inherit_temp(&PathBuf::from(t)));

        let component = std::env::var(Self::get_component_key())
            .ok()
            .map(|c| c.try_into())
            .transpose()
            .context("parse component reference")
            .unwrap();

        let tmp_dir = Arc::new(OnceCell::new_with(tmp_dir));

        let s = Self {
            parents,
            tmp_dir,
            component,
            tmp,
            context_id: uuid::Uuid::new_v4().to_string(),
        };

        tokio::spawn({
            let s = s.clone();
            async move {
                let temp = s
                    .get_tmp()
                    .await
                    .map(|t| t.to_string())
                    .ok()
                    .unwrap_or("no temp dir found".to_string());

                tracing::debug!(
                    context_id = s.context_id,
                    parents = s.get_parents().join(","),
                    inherited = s.inherited(),
                    tmp_dir = temp,
                    component = s
                        .component()
                        .as_ref()
                        .map(|t| t.to_string())
                        .unwrap_or("".to_string()),
                    "loaded context"
                )
            }
        });

        s
    }

    pub const fn get_context_key() -> &'static str {
        "NON_CONTEXT"
    }
    pub fn get_parents(&self) -> &[String] {
        &self.parents
    }

    pub fn inherited(&self) -> bool {
        !self.get_parents().is_empty()
    }

    pub fn append_context_id(&self) -> Vec<String> {
        let mut parents = self.parents.clone();
        parents.push(self.context_id.clone());
        parents
    }

    pub fn context_string(&self) -> String {
        self.append_context_id().join(",").to_string()
    }

    pub const fn get_tmp_key() -> &'static str {
        "NON_TMP"
    }
    pub async fn get_tmp(&self) -> anyhow::Result<TempDirectory> {
        let tmp = self.tmp.clone();
        let tmp_dir = self
            .tmp_dir
            .get_or_try_init(|| async move {
                let dir = tmp.create_temp().await?;

                Ok::<_, anyhow::Error>(dir)
            })
            .await?;

        Ok(tmp_dir.clone())
    }

    pub const fn get_component_key() -> &'static str {
        "NON_COMPONENT"
    }

    pub fn component(&self) -> &Option<ComponentReference> {
        &self.component
    }
}

pub trait NonContextState {
    fn context(&self) -> NonContext;
}

impl NonContextState for State {
    fn context(&self) -> NonContext {
        static ONCE: OnceLock<NonContext> = OnceLock::new();

        ONCE.get_or_init(|| NonContext::from_env(self.temp_directories()))
            .clone()
    }
}
