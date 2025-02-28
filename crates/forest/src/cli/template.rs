use std::path::Path;

use tokio::io::AsyncWriteExt;

use crate::model::{Context, TemplateType};

#[derive(clap::Parser)]
pub struct Template {}

impl Template {
    pub async fn execute(self, project_path: &Path, context: &Context) -> anyhow::Result<()> {
        tracing::info!("templating");

        self.execute_plan(project_path, context).await?;
        self.execute_project(project_path, context).await?;

        Ok(())
    }

    async fn execute_plan(&self, project_path: &Path, context: &Context) -> anyhow::Result<()> {
        let plan_path = &project_path.join(".forest").join("plan");

        let Some(Some(template)) = &context.plan.as_ref().map(|p| &p.templates) else {
            return Ok(());
        };

        match template.ty {
            TemplateType::Jinja2 => {
                for entry in glob::glob(&format!(
                    "{}/{}",
                    plan_path.display().to_string().trim_end_matches("/"),
                    template.path.trim_start_matches("./"),
                ))
                .map_err(|e| anyhow::anyhow!("failed to read glob pattern: {}", e))?
                {
                    let entry = entry.map_err(|e| anyhow::anyhow!("failed to read path: {}", e))?;
                    let entry_name = entry.display().to_string();

                    let entry_rel = if entry.is_absolute() {
                        entry.strip_prefix(plan_path).map(|e| e.to_path_buf())
                    } else {
                        Ok(entry.clone())
                    };

                    let rel_file_path = entry_rel
                        .map(|p| {
                            if p.file_name()
                                .map(|f| f.to_string_lossy().ends_with(".jinja2"))
                                .unwrap_or(false)
                            {
                                p.with_file_name(
                                    p.file_stem().expect("to be able to find a filename"),
                                )
                            } else {
                                p.to_path_buf()
                            }
                        })
                        .map_err(|e| {
                            anyhow::anyhow!(
                                "failed to find relative file: {}, project: {}, file: {}",
                                e,
                                plan_path.display(),
                                entry_name
                            )
                        })?;

                    let output_file_path = project_path
                        .join(".forest/temp")
                        .join(&template.output)
                        .join(rel_file_path);

                    let contents = tokio::fs::read_to_string(&entry).await.map_err(|e| {
                        anyhow::anyhow!("failed to read template: {}, err: {}", entry.display(), e)
                    })?;

                    let mut env = minijinja::Environment::new();
                    env.add_template(&entry_name, &contents)?;
                    env.add_global("global", &context.project.global);

                    let tmpl = env.get_template(&entry_name)?;

                    let output = tmpl
                        .render(minijinja::context! {})
                        .map_err(|e| anyhow::anyhow!("failed to render template: {}", e))?;

                    tracing::info!("rendered template: {}", output);

                    if let Some(parent) = output_file_path.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(|e| {
                            anyhow::anyhow!(
                                "failed to create directory (path: {}) for output: {}",
                                parent.display(),
                                e
                            )
                        })?;
                    }

                    let mut output_file = tokio::fs::File::create(&output_file_path)
                        .await
                        .map_err(|e| {
                            anyhow::anyhow!(
                                "failed to create file: {}, error: {}",
                                output_file_path.display(),
                                e
                            )
                        })?;
                    output_file.write_all(output.as_bytes()).await?;
                }
            }
        }

        Ok(())
    }
    async fn execute_project(&self, project_path: &Path, context: &Context) -> anyhow::Result<()> {
        let Some(template) = &context.project.templates else {
            return Ok(());
        };

        match template.ty {
            TemplateType::Jinja2 => {
                for entry in glob::glob(&format!(
                    "{}/{}",
                    project_path.display().to_string().trim_end_matches("/"),
                    template.path.trim_start_matches("./"),
                ))
                .map_err(|e| anyhow::anyhow!("failed to read glob pattern: {}", e))?
                {
                    let entry = entry.map_err(|e| anyhow::anyhow!("failed to read path: {}", e))?;
                    let entry_name = entry.display().to_string();

                    let entry_rel = if entry.is_absolute() {
                        entry.strip_prefix(project_path).map(|e| e.to_path_buf())
                    } else {
                        Ok(entry.clone())
                    };

                    let rel_file_path = entry_rel
                        .map(|p| {
                            if p.file_name()
                                .map(|f| f.to_string_lossy().ends_with(".jinja2"))
                                .unwrap_or(false)
                            {
                                p.with_file_name(
                                    p.file_stem().expect("to be able to find a filename"),
                                )
                            } else {
                                p.to_path_buf()
                            }
                        })
                        .map_err(|e| {
                            anyhow::anyhow!(
                                "failed to find relative file: {}, project: {}, file: {}",
                                e,
                                project_path.display(),
                                entry_name
                            )
                        })?;

                    let output_file_path = project_path
                        .join(".forest/temp")
                        .join(&template.output)
                        .join(rel_file_path);

                    let contents = tokio::fs::read_to_string(&entry).await.map_err(|e| {
                        anyhow::anyhow!("failed to read template: {}, err: {}", entry.display(), e)
                    })?;

                    let mut env = minijinja::Environment::new();
                    env.add_template(&entry_name, &contents)?;
                    env.add_global("global", &context.project.global);

                    let tmpl = env.get_template(&entry_name)?;

                    let output = tmpl
                        .render(minijinja::context! {})
                        .map_err(|e| anyhow::anyhow!("failed to render template: {}", e))?;

                    tracing::info!("rendered template: {}", output);

                    if let Some(parent) = output_file_path.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(|e| {
                            anyhow::anyhow!(
                                "failed to create directory (path: {}) for output: {}",
                                parent.display(),
                                e
                            )
                        })?;
                    }

                    let mut output_file = tokio::fs::File::create(&output_file_path)
                        .await
                        .map_err(|e| {
                            anyhow::anyhow!(
                                "failed to create file: {}, error: {}",
                                output_file_path.display(),
                                e
                            )
                        })?;
                    output_file.write_all(output.as_bytes()).await?;
                }
            }
        }

        Ok(())
    }
}
