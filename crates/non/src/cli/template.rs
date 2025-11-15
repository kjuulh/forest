use std::path::PathBuf;

use anyhow::Context;
use tokio::io::AsyncWriteExt;

use crate::{
    models::ProjectValue,
    non_context::NonContextState,
    services::{
        component_parser::ComponentParserState, components::ComponentsServiceState,
        project::ProjectParserState,
    },
    state::State,
};

#[derive(clap::Parser)]
pub struct TemplateCommand {
    files: Vec<PathBuf>,
}

impl TemplateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let ctx = state.context();
        let project = state.project_parser().get_project().await?;

        let templates_path = if let Some(component_ref) = ctx.component() {
            let comps = state.components_service();

            let component = comps.get_local_component(component_ref).await?;

            let raw = state.component_parser().parse(&component.path).await?;

            raw.path.join("templates")
        } else {
            project.path.join("templates")
        };

        let config = match ctx.component() {
            Some(component_ref) => project.get_component_config(component_ref),
            None => todo!("project templating not supported yet"),
        };

        let temp_dir = ctx.get_tmp().await?;

        // Run template(s) needed

        for file in &self.files {
            tracing::debug!("processing file: {}", file.to_string_lossy());

            let template_file = templates_path.join(file);

            if !template_file.exists() {
                anyhow::bail!("file does not exist: {}", template_file.to_string_lossy());
            }

            if !template_file.is_file() {
                anyhow::bail!("path is not a file: {}", template_file.to_string_lossy());
            }

            let Some(file_name) = template_file.file_name() else {
                continue;
            };

            let new_file_name = file_name.to_str().unwrap().trim_end_matches(".jinja2");
            let mut new_file_path = temp_dir.join(file);
            new_file_path.set_file_name(new_file_name);

            if let Some(parent) = new_file_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .context(anyhow::anyhow!(
                        "create template file dir: {}",
                        parent.display()
                    ))?;
            }

            if let Some("jinja2") = template_file.extension().and_then(|f| f.to_str()) {
                tracing::info!(from = %template_file.display(), to = %new_file_path.display(), "templating file");

                let raw_content = tokio::fs::read_to_string(template_file)
                    .await
                    .context("read template file")?;
                let rendered_content =
                    render_template(&raw_content, &config).context("render template")?;

                let mut file = tokio::fs::File::create(&new_file_path)
                    .await
                    .context("create template file")?;
                file.write_all(rendered_content.as_bytes())
                    .await
                    .context("write template content")?;
                file.flush().await.context("flush file")?;
            } else {
                tracing::info!(from = %template_file.display(), to = %new_file_path.display(), "copying file");

                tokio::fs::copy(&template_file, &new_file_path)
                    .await
                    .context(anyhow::anyhow!(
                        "copying file: from {} - to {}",
                        template_file.display(),
                        new_file_path.display()
                    ))?;
            }
        }

        // Output to tmp destination

        Ok(())
    }
}

fn render_template(
    template_content: &str,
    config: &Option<&ProjectValue>,
) -> anyhow::Result<String> {
    let mut env = minijinja::Environment::new();
    // Debug diagnostics, jinja is not fun to debug without
    env.set_debug(true);

    if let Some(config) = config {
        env.add_global("config", minijinja::Value::from_serialize(config));
    }

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
    env.add_filter(
        "as_bool",
        |input: String| -> Result<bool, minijinja::Error> {
            input.parse::<bool>().map_err(|e| {
                minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
            })
        },
    );

    let res = env.render_str(template_content, minijinja::context! {});

    match res {
        Ok(output) => Ok(output),
        Err(e) => {
            let mut error_causes = Vec::new();

            error_causes.push(format!("template error: {e:#}\n\n"));

            let mut err = &e as &dyn std::error::Error;
            while let Some(next_err) = err.source() {
                error_causes.push(format!("caused by: {:#}", next_err));
                err = next_err;
            }

            anyhow::bail!("{}", error_causes.join("\n\n"));
        }
    }
}
