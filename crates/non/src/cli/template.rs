use std::path::PathBuf;

use anyhow::Context;

use crate::{
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

        let templates_path = if let Some(component_ref) = ctx.component() {
            let comps = state.components_service();

            let component = comps.get_local_component(component_ref).await?;

            let raw = state.component_parser().parse(&component.path).await?;

            raw.path.join("templates")
        } else {
            let project = state.project_parser().get_project().await?;

            project.path.join("templates")
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

            if let Some("jinja2") = template_file.extension().and_then(|f| f.to_str()) {
                todo!("templating not implemented yet");
            }

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

            tracing::info!(from = %template_file.display(), to = %new_file_path.display(), "templating file");
            tokio::fs::copy(&template_file, &new_file_path)
                .await
                .context(anyhow::anyhow!(
                    "copying file: from {} - to {}",
                    template_file.display(),
                    new_file_path.display()
                ))?;
        }

        // Output to tmp destination

        Ok(())
    }
}
