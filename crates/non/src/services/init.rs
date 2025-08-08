use std::path::Path;

use crate::state::State;

pub mod models;

use anyhow::Context;
use models::*;
use noworkers::extensions::WithSysLimitCpus;

use super::{
    components::{ComponentsService, ComponentsServiceState},
    temp_directories::{TempDirectories, TempDirectoriesState},
};

pub struct InitService {
    components: ComponentsService,

    temp: TempDirectories,
}

impl InitService {
    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn init(&self, choice: &Option<String>, dest: &Path) -> anyhow::Result<()> {
        let sources = self.fetch_sources().await?;

        let Some(choice) = self.get_choice(&sources, choice).await? else {
            tracing::warn!("user choice was not found in list of items");
            anyhow::bail!("failed to find source");
        };

        let template = self.render_choice(&choice).await?;

        self.move_template(&template, dest).await?;

        Ok(())
    }

    pub async fn fetch_sources(&self) -> anyhow::Result<Choices> {
        tracing::debug!("fetching init sources");

        self.components.sync_components().await?;
        let inits = self.components.get_inits().await?;

        Ok(Choices {
            choices: inits
                .into_iter()
                .map(|(k, (init_key, value))| Choice {
                    name: k,
                    init: init_key,
                    component: value,
                })
                .collect(),
        })
    }

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn get_choice(
        &self,
        choices: &Choices,
        choice: &Option<String>,
    ) -> anyhow::Result<Option<Choice>> {
        tracing::debug!("providing user choice of source");

        if choices.choices.is_empty() {
            anyhow::bail!(
                "No choices available, add some projects first `non global add <my-init-project>`"
            )
        }

        let user_choice = match choice {
            Some(user_choice) => user_choice.clone(),
            None => inquire::Select::new(
                "choose a template to bootstrap your project",
                choices.to_string_vec(),
            )
            .with_vim_mode(true)
            .prompt()?,
        };

        let Some(choice) = choices.get(&user_choice) else {
            tracing::warn!(user_choice, "failed to find choice");

            return Ok(None);
        };

        Ok(Some(choice))
    }

    pub async fn render_choice(&self, choice: &Choice) -> anyhow::Result<Template> {
        tracing::debug!("fetching template into temp");

        let temp = self.temp.create_emphemeral_temp().await?;

        let init = choice
            .component
            .init
            .get(&choice.init)
            .expect("item from choice to match internal structure");

        println!("choice: {}", choice.name);

        let component_path = self
            .components
            .get_component_path(&choice.component)
            .await?;

        let init_path = component_path.join("init").join(&choice.init).join("files");

        copy(&init_path, &temp)
            .await
            .context("copy init for render choice")?;

        Ok(Template { path: temp })
    }

    pub async fn move_template(&self, template: &Template, dest: &Path) -> anyhow::Result<()> {
        tracing::debug!("putting template in path");

        copy(&template.path, dest)
            .await
            .context("copy files to final destination")?;

        // TODO: move template files into current dir

        Ok(())
    }
}

async fn copy(src: &Path, dest: &Path) -> anyhow::Result<()> {
    tracing::debug!(
        src = src.display().to_string(),
        dest = dest.display().to_string(),
        "copying files"
    );
    let mut file_entries = Vec::new();

    for path in walkdir::WalkDir::new(src) {
        let path = path?;

        if !path.file_type().is_file() {
            continue;
        }

        file_entries.push(path);
    }

    let mut workers = noworkers::Workers::new();

    workers.with_limit_to_system_cpus();

    for file_entry in file_entries {
        workers
            .add({
                let src = src.to_path_buf();
                let dest = dest.to_path_buf();

                move |_| async move {
                    let src_path = file_entry.path();
                    let rel_path = src_path
                        .strip_prefix(src)
                        .context("strip prefix from src file")?;
                    let dest_path = dest.join(rel_path);

                    if let Some(parent) = dest_path.parent() {
                        tokio::fs::create_dir_all(parent)
                            .await
                            .context("create dir for copy")?;
                    }

                    tracing::trace!(
                        src = src_path.display().to_string(),
                        dest = dest_path.display().to_string(),
                        "copy file"
                    );

                    tokio::fs::copy(src_path, dest_path)
                        .await
                        .context("copy file")?;

                    Ok(())
                }
            })
            .await?;
    }

    workers.wait().await?;

    tracing::debug!(
        src = src.display().to_string(),
        dest = dest.display().to_string(),
        "copied files"
    );

    Ok(())
}

pub trait InitServiceState {
    fn init_service(&self) -> InitService;
}

impl InitServiceState for State {
    fn init_service(&self) -> InitService {
        InitService {
            components: self.components_service(),
            temp: self.temp_directories(),
        }
    }
}
