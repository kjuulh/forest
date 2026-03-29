use std::fmt::Display;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::{
    contracts::{self, EnabledContracts},
    forest_context::ForestContextState,
    models::{ComponentReference, ProjectValue},
    services::{
        component_binary, project::ProjectParserState,
        templates::TemplatesServiceState,
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

        // Derive available contracts from project dependencies
        let enabled_contracts = EnabledContracts::from_project_dependencies(&project);
        enabled_contracts.require(contracts::CONTRACT_DEPLOYMENT)?;

        tracing::info!("enabled contracts: {}", enabled_contracts);

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
            tracing::trace!("adding deployment from dependencies");

            get_deployment_items(
                &mut deployment_items,
                Some(component_ref),
                component_config,
                envs,
            )?;
        }

        // Local deployment
        //

        if let ProjectValue::Map(keys) = project.other
            && let Some(ProjectValue::Map(project)) = keys.get("project")
            && let Some(ProjectValue::Map(envs)) = project.get("env")
        {
            let project_config = project.get("config");
            tracing::trace!("adding deployment from project");
            get_deployment_items(&mut deployment_items, None, project_config, envs)?;
        }

        tracing::info!("generate deployment env");

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
                component = deployment_item.component.as_ref().map(|c| c.to_string()),
                "prepare deployment item",
            );

            let output_path = deployment_output
                .join(&deployment_item.env)
                .join(&deployment_item.destination)
                .join(&deployment_item.destination_type);

            let Some(component) = &deployment_item.component else {
                anyhow::bail!(
                    "deployment item for {}/{} has no component reference",
                    deployment_item.env, deployment_item.destination
                );
            };

            let crate::models::ComponentSource::Local(component_path) = &component.source else {
                anyhow::bail!(
                    "component {}/{} must be a local dependency for release prepare",
                    component.organisation, component.name
                );
            };

            tokio::fs::create_dir_all(&output_path)
                .await
                .context("create output dir")?;

            // ── Pass 1: Copy/render component templates ──
            // Components ship template files in templates/deployment/{destination_type}/
            // These are copied to the output, with .jinja2 files rendered using config.
            // The component binary can customize rendering via _meta/template_config.
            let template_dir = component_path
                .join("templates")
                .join("deployment")
                .join(&deployment_item.destination_type);

            if template_dir.exists() {
                // Get template config from the component binary (skip, rename, extra vars)
                let template_config = if let Some(ref bp) = component_binary::resolve_binary(component_path, &component.name) {
                    component_binary::get_template_config(bp).await.unwrap_or_default()
                } else {
                    forest_sdk::TemplateConfig::default()
                };

                tracing::info!(
                    "rendering templates from {} for {}/{}{}",
                    template_dir.display(),
                    deployment_item.env,
                    deployment_item.destination,
                    if !template_config.skip.is_empty() {
                        format!(" (skipping: {:?})", template_config.skip)
                    } else {
                        String::new()
                    },
                );

                state
                    .templates_service()
                    .template_folder(
                        &template_dir,
                        &output_path,
                        &deployment_item.config.as_ref(),
                    )
                    .await
                    .context("render deployment templates")?;

                // Apply skip rules — remove files matching skip patterns
                for pattern in &template_config.skip {
                    let glob_pattern = output_path.join(pattern).to_string_lossy().to_string();
                    for entry in glob::glob(&glob_pattern).into_iter().flatten().flatten() {
                        if entry.is_file() {
                            let _ = tokio::fs::remove_file(&entry).await;
                            tracing::debug!("skipped: {}", entry.display());
                        }
                    }
                }
            }

            // ── Pass 2: Invoke deployment hook (optional) ──
            // If the component has a binary or Deno runtime with deployment hooks,
            // call prepare to get additional manifest files.
            let hook_result = {
                let spec_json = deployment_item
                    .config
                    .as_ref()
                    .map(|c| serde_json::to_value(c).unwrap_or_default())
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let empty_input = serde_json::Value::Object(serde_json::Map::new());
                let call_context = forest_sdk::CallContext {
                    project: Some(project.name.clone()),
                    organisation: project.organisation.clone(),
                    environment: Some(deployment_item.env.clone()),
                    work_dir: Some(project.path.to_string_lossy().to_string()),
                    ..Default::default()
                };

                if let Some(binary_path) = component_binary::resolve_binary(component_path, &component.name) {
                    tracing::info!("invoking deployment prepare hook on {}/{}", component.organisation, component.name);
                    Some(component_binary::invoke_component_with_context(
                        &binary_path,
                        "hooks/forest/deployment/prepare",
                        &spec_json,
                        &empty_input,
                        Some(&call_context),
                    ).await.with_context(|| format!(
                        "deployment prepare hook failed for {}/{}",
                        component.organisation, component.name
                    ))?)
                } else if crate::services::component_deno::is_deno_component(component_path) {
                    if let Some(entrypoint) = crate::services::component_deno::resolve_entrypoint(component_path) {
                        tracing::info!("invoking deno deployment prepare hook on {}/{}", component.organisation, component.name);
                        Some(crate::services::component_deno::invoke_deno_component(
                            component_path,
                            &entrypoint,
                            "hooks/forest/deployment/prepare",
                            &spec_json,
                            &empty_input,
                            Some(&call_context),
                        ).await.with_context(|| format!(
                            "deno deployment prepare hook failed for {}/{}",
                            component.organisation, component.name
                        ))?)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            // Write manifests returned by the hook.
            // Each manifest is { name: "filename.yaml", content: "..." }.
            if let Some(result) = hook_result {
                if let Some(manifests) = result.get("manifests").and_then(|v| v.as_array()) {
                    for manifest in manifests {
                        let obj = manifest.as_object()
                            .context("manifest must be an object with name and content")?;
                        let name = obj.get("name").and_then(|n| n.as_str())
                            .context("manifest.name is required")?;
                        let content = obj.get("content").and_then(|c| c.as_str())
                            .context("manifest.content is required")?;

                        let file_path = output_path.join(name);
                        tokio::fs::write(&file_path, content)
                            .await
                            .with_context(|| format!("write manifest {name}"))?;
                        tracing::info!("wrote manifest: {}", file_path.display());
                    }
                }
            }

            // Write forest config.json
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

            tracing::info!("generated deployment at: {}", output_path.display());
        }

        Ok(())
    }
}

fn get_deployment_items(
    deployment_items: &mut Vec<DeploymentItem>,
    component_ref: Option<ComponentReference>,
    config: Option<&ProjectValue>,
    envs: &std::collections::HashMap<String, ProjectValue>,
) -> anyhow::Result<()> {
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
        let config = match (config, config_values) {
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

            let Some(ProjectValue::String(dest)) = destination_config.get("destination") else {
                anyhow::bail!("item is required to have a destination");
            };

            let Some(ProjectValue::String(destination_type)) = destination_config.get("type")
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
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentItem {
    env: String,
    destination: String,
    destination_type: String,
    component: Option<ComponentReference>,
    config: Option<ProjectValue>,
}

impl Display for DeploymentItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.component {
            Some(component) => {
                write!(
                    f,
                    "{}/{}/{} (component={})",
                    self.env, self.destination, self.destination_type, component
                )
            }
            None => {
                write!(
                    f,
                    "{}/{}/{}",
                    self.env, self.destination, self.destination_type
                )
            }
        }
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
