use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crate::state::State;

pub mod models;

use anyhow::Context;
use models::*;
use noworkers::extensions::WithSysLimitCpus;
use tokio::io::AsyncWriteExt;

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

        self.components.sync_components(None).await?;
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

        // let init = choice
        //     .component
        //     .init
        //     .get(&choice.init)
        //     .expect("item from choice to match internal structure");

        // let choices = self.collect_input(choice).await?;
        // println!("choice: {}", choice.name);

        // if !choices.is_empty() {
        //     for (k, v) in &choices {
        //         println!("  - {k}: {v}");
        //     }
        // }

        let component_path = self
            .components
            .get_component_path(&choice.component)
            .await?;

        let init_path = component_path.join("init").join(&choice.init).join("files");

        copy(&init_path, &temp)
            .await
            .context("copy init for render choice")?;

        // self.apply_templates(&temp, &choices)
        //     .await
        //     .context("apply templates")?;

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

    // #[tracing::instrument(skip(self), level = "trace")]
    // async fn collect_input(
    //     &self,
    //     init: &crate::component_cache::models::Init,
    // ) -> anyhow::Result<BTreeMap<String, String>> {
    //     tracing::debug!("collecting input from user");
    //     let mut inputs = BTreeMap::new();

    //     for (input_name, input_requirements) in &init.input {
    //         let prompt =
    //             inquire::Text::new(input_name).with_initial_value(&input_requirements.default);

    //         let prompt = if let Some(desc) = &input_requirements.description {
    //             prompt.with_help_message(desc)
    //         } else {
    //             prompt
    //         };

    //         let output = if input_requirements.required {
    //             let output = prompt.prompt()?;
    //             if output.is_empty() {
    //                 anyhow::bail!("{} is required", input_name)
    //             }

    //             output
    //         } else {
    //             prompt
    //                 .prompt_skippable()?
    //                 .unwrap_or_else(|| input_requirements.default.clone())
    //         };

    //         inputs.insert(input_name.clone(), output);
    //     }

    //     Ok(inputs)
    // }

    async fn apply_templates(
        &self,
        temp: &super::temp_directories::GuardedTempDirectory,
        choices: &BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        let mut file_entries = Vec::new();

        for path in walkdir::WalkDir::new(temp.as_path()) {
            let path = path?;

            if !path.file_type().is_file() {
                continue;
            }

            file_entries.push(path);
        }

        let mut workers = noworkers::Workers::new();

        workers.with_limit_to_system_cpus();

        for entry in file_entries {
            if let Some(ext) = entry.path().extension()
                && ext == "jinja"
            {
                let choices = choices.clone();

                workers
                    .add(move |_| async move {
                        let entry_path = entry.path();
                        let template_content = tokio::fs::read_to_string(&entry_path)
                            .await
                            .context("read template")?;

                        let action =
                            apply_template(&template_content, choices).context("apply template")?;
                        let output = match action {
                            TemplateAction::Skip => {
                                tracing::warn!("skipping file: {}", entry_path.display());
                                return Ok(());
                            }
                            TemplateAction::Run { output } => output,
                        };

                        let parent = entry_path
                            .parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_else(|| PathBuf::from("."));
                        let new_file_path = parent
                            .join(entry_path.file_stem().expect("to be able to get file stem"));

                        let mut file = tokio::fs::File::create_new(new_file_path).await?;
                        file.write_all(output.as_bytes())
                            .await
                            .context("write template file")?;
                        file.flush().await.context("flush template file")?;

                        tokio::fs::remove_file(entry.path())
                            .await
                            .context("remove template file")?;

                        Ok(())
                    })
                    .await?;
            }
        }

        workers.wait().await?;

        Ok(())
    }
}

fn apply_template(
    template_content: &str,
    choices: BTreeMap<String, String>,
) -> anyhow::Result<TemplateAction> {
    let mut env = minijinja::Environment::new();
    env.add_global("input", choices);
    env.add_filter("to_lower", |input: String| -> String {
        input.to_lowercase()
    });
    env.add_filter("to_upper", |input: String| -> String {
        input.to_uppercase()
    });
    env.add_filter("to_snake", |input: String| -> String {
        stringcase::snake_case(&input)
    });
    env.add_filter("to_camel", |input: String| -> String {
        stringcase::camel_case(&input)
    });
    env.add_filter("to_pascal", |input: String| -> String {
        stringcase::pascal_case(&input)
    });
    env.add_filter("to_screaming_snake", |input: String| -> String {
        stringcase::macro_case(&input)
    });
    env.add_filter("to_kebab", |input: String| -> String {
        stringcase::kebab_case(&input)
    });

    let file_ignore: Arc<Mutex<bool>> = Arc::default();
    env.add_function("ignore_file", {
        let file_ignore = file_ignore.clone();

        move |input: bool| {
            let mut file_ignore = file_ignore.lock().unwrap();
            *file_ignore = input;
        }
    });

    let output = env
        .render_str(template_content, minijinja::context! {})
        .context("render template for init")?;

    if *file_ignore.lock().unwrap() {
        return Ok(TemplateAction::Skip);
    }

    Ok(TemplateAction::Run { output })
}

enum TemplateAction {
    Skip,
    Run { output: String },
}

#[tracing::instrument(level = "trace")]
async fn copy(src: &Path, dest: &Path) -> anyhow::Result<()> {
    tracing::debug!("copying files");
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

    tracing::debug!("copied files");

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

#[cfg(test)]
mod test {}
