use std::fmt::Display;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::{
    models::{ComponentReference, ProjectValue},
    forest_context::NonContextState,
    services::{
        component_parser::ComponentParserState, components::ComponentsServiceState,
        project::ProjectParserState, templates::TemplatesServiceState,
    },
    state::State,
};

#[derive(clap::Parser)]
pub struct PrepareCommand {}

impl PrepareCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        if state.context().inherited() {
            anyhow::bail!("deploy prepare should never be called from a component");
        }

        // Parse project
        //

        let project = state.project_parser().get_project().await?;

        let Some(deployment_component) = project.dependencies.get("forest", "deployment") else {
            anyhow::bail!("deployment component isn't added, deployment prepare aborts");
        };

        // Get Deployment values
        //

        let Some(ProjectValue::Map(deployment_values)) =
            project.other.get_from_component(&deployment_component)
        else {
            tracing::info!("skipping, deployment as config hasn't been specified");

            return Ok(());
        };

        let Some(ProjectValue::Bool(enabled)) = deployment_values.get("enabled") else {
            anyhow::bail!("forest.deployment.enabled is not specified");
        };

        // Deployment is enabled
        //

        if !enabled {
            tracing::info!("deployment has been disabled skipping");

            return Ok(());
        }

        // Get deployment for all dependencies
        //

        let mut deployment_items = Vec::new();

        for component_ref in project.dependencies.get_components() {
            let Some(ProjectValue::Map(component)) =
                project.other.get_from_component(&component_ref)
            else {
                tracing::trace!("no config available component");
                continue;
            };

            let component_config = component.get("config");
            let Some(ProjectValue::Map(envs)) = component.get("env") else {
                tracing::trace!("env is not set for component");
                continue;
            };

            if envs.is_empty() {
                tracing::trace!("no environment selected, skipping");
                continue;
            }

            for (env, value) in envs {
                let ProjectValue::Map(env_config) = value else {
                    tracing::trace!("env is required to be a map");
                    continue;
                };

                let Some(ProjectValue::Array(destinations)) = env_config.get("destinations") else {
                    tracing::trace!("destinations key is required for env config");
                    continue;
                };

                let config_values = env_config.get("config");

                // Merge config values
                let config = match (component_config, config_values) {
                    (None, None) => None,
                    (None, Some(config)) => Some(config.clone()),
                    (Some(config), None) => Some(config.clone()),
                    (Some(base_config), Some(patch_config)) => {
                        // Override the base config with the path

                        let config = merge_config(base_config.clone(), patch_config.clone());

                        Some(config)
                    }
                };

                for destination in destinations {
                    let ProjectValue::Map(destination_config) = destination else {
                        anyhow::bail!("destination is required to be a map");
                    };

                    let Some(ProjectValue::String(dest)) = destination_config.get("destination")
                    else {
                        anyhow::bail!("item is required to have a destination");
                    };

                    let Some(ProjectValue::String(destination_type)) =
                        destination_config.get("type")
                    else {
                        anyhow::bail!("destination is required to have a type");
                    };

                    // We've got a component, env, destination, type
                    deployment_items.push(DeploymentItem {
                        env: env.clone(),
                        destination: dest.clone(),
                        destination_type: destination_type.clone(),
                        component: component_ref.clone(),
                        config: config.clone(),
                    })
                }

                tracing::info!("generate deployment env")
            }
        }

        let deployment_output = project.path.join(".forest").join("deployment");
        // CHORE: Maybe keep, maybe remove .join(uuid::Uuid::now_v7().to_string());
        if deployment_output.exists() {
            tokio::fs::remove_dir_all(&deployment_output).await?;
        }

        tokio::fs::create_dir_all(&deployment_output)
            .await
            .context("create deployment preparation output")?;

        // For each deployment item, send deployment prepare to component in question
        for deployment_item in deployment_items {
            tracing::info!(
                env = deployment_item.env,
                destination = deployment_item.destination,
                destination_type = deployment_item.destination_type,
                component = deployment_item.component.to_string(),
                "parepare deployment item",
            );

            // 1. Go to component
            // TODO: handle local deployment
            let comp = state
                .components_service()
                .get_local_component(&deployment_item.component)
                .await
                .context("failed to get component")?;

            let raw = state.component_parser().parse(&comp.path).await?;

            // Now we build the template path
            let input_path = raw
                .path
                .join("templates")
                .join("deployment")
                .join(&deployment_item.destination_type);
            if !input_path.exists() {
                anyhow::bail!(
                    "path: {} does not exist, cannot prepare deployment",
                    input_path.display()
                )
            }
            let output_path = deployment_output
                .join(&deployment_item.env)
                .join(&deployment_item.destination)
                .join(&deployment_item.destination_type);

            state
                .templates_service()
                .template_folder(&input_path, &output_path, &deployment_item.config.as_ref())
                .await
                .context("prepare deployment: templates")?;

            let config_file_path = output_path.join("forest").join("config.json");
            if let Some(parent) = config_file_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .context("forest config file path")?;
            }
            let mut config_file = tokio::fs::File::create(config_file_path)
                .await
                .context("create config file")?;
            config_file
                .write_all(
                    serde_json::to_string_pretty(&deployment_item)
                        .context("serialize deployment item")?
                        .as_bytes(),
                )
                .await
                .context("write forest config")?;
            config_file.flush().await?;

            tracing::info!("generated templates at: {}", output_path.display());
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentItem {
    env: String,
    destination: String,
    destination_type: String,
    component: ComponentReference,
    config: Option<ProjectValue>,
}

impl Display for DeploymentItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}/{} (component={})",
            self.env, self.destination, self.destination_type, self.component
        )
    }
}

fn merge_config(base: ProjectValue, patch: ProjectValue) -> ProjectValue {
    match (base, patch) {
        (ProjectValue::String(_base), ProjectValue::String(patch)) => ProjectValue::String(patch),
        (ProjectValue::Integer(_base), ProjectValue::Integer(patch)) => {
            ProjectValue::Integer(patch)
        }
        (ProjectValue::Decimal(_base), ProjectValue::Decimal(patch)) => {
            ProjectValue::Decimal(patch)
        }
        (ProjectValue::Bool(_base), ProjectValue::Bool(patch)) => ProjectValue::Bool(patch),
        (ProjectValue::Map(base), ProjectValue::Map(patch)) => {
            let mut base = base.clone();

            for (patch_key, patch_val) in patch {
                match base.get_mut(&patch_key) {
                    Some(base_value) => {
                        let patch_val = merge_config(base_value.clone(), patch_val);
                        base.insert(patch_key, patch_val);
                    }
                    None => {
                        base.insert(patch_key, patch_val);
                    }
                }
            }

            ProjectValue::Map(base)
        }
        (ProjectValue::Array(base), ProjectValue::Array(patch)) => {
            let mut base = base.clone();

            for p in patch {
                base.push(p);
            }

            ProjectValue::Array(base)
        }
        (_, value) => {
            // If the type doesn't match, we simply override the entire value
            value
        }
    }
}
