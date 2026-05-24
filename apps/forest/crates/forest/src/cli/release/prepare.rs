use std::fmt::Display;
use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::{
    component_cache::ComponentCacheState,
    contracts::{self, EnabledContracts},
    forest_context::ForestContextState,
    models::{ComponentReference, ProjectValue},
    services::{
        component_binary, component_deno, components::ComponentsServiceState,
        project::ProjectParserState, templates::TemplatesServiceState,
    },
    state::State,
};

#[derive(clap::Parser)]
pub struct PrepareCommand {
    /// Override config values. Format: org/component.key=value
    /// Example: --set kjuulh/service.tag=abc123
    /// Nested keys use dots: --set kjuulh/service.env_vars.LOG_LEVEL=debug
    #[arg(long = "set", value_name = "KEY=VALUE")]
    pub overrides: Vec<String>,
}

impl PrepareCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        if state.context().inherited() {
            anyhow::bail!("deploy prepare should never be called from a component");
        }

        // Parse project
        //

        let mut project = state.project_parser().get_project().await?;

        // Apply --set overrides to project config
        for kv in &self.overrides {
            apply_config_override(&mut project.other, kv)
                .with_context(|| format!("invalid --set value: {kv}"))?;
        }

        // Derive available contracts from project dependencies
        let enabled_contracts = EnabledContracts::from_project_dependencies(&project);
        enabled_contracts.require(contracts::CONTRACT_DEPLOYMENT)?;

        tracing::info!("enabled contracts: {}", enabled_contracts);

        // Auto-resolve missing dependencies (cargo-build-style). Versioned
        // deps that aren't already in the cache get downloaded here, so
        // `release prepare` works on a clean checkout without the user
        // having to remember to run `forest deps` first.
        state
            .components_service()
            .get_components_project(project.clone())
            .await
            .context("auto-resolving project dependencies")?;

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

        // Build a resolver for inter-component calls
        let call_resolver = {
            let mut component_map: std::collections::HashMap<String, (std::path::PathBuf, String)> =
                std::collections::HashMap::new();
            for dep in project.dependencies.get_components() {
                let component_id = format!("{}/{}", dep.organisation, dep.name);
                match &dep.source {
                    crate::models::ComponentSource::Local(path) => {
                        if component_deno::is_deno_component(path) {
                            if let Some(entrypoint) = component_deno::resolve_entrypoint(path) {
                                component_map.insert(component_id, (path.clone(), entrypoint));
                            }
                        }
                    }
                    crate::models::ComponentSource::Versioned(version) => {
                        // Versioned deps materialize in the shared cache via
                        // `forest update`. Resolve to that cache path and
                        // reuse the same is_deno detection used by Local.
                        let Some(cache_dir) = dirs::cache_dir() else { continue };
                        let dep_path = cache_dir
                            .join("forest")
                            .join("components")
                            .join(&dep.organisation)
                            .join(&dep.name)
                            .join(version.to_string());
                        if component_deno::is_deno_component_with_meta(
                            &dep_path,
                            Some(&dep.organisation),
                            Some(&dep.name),
                            Some(&version.to_string()),
                        ) {
                            if let Some(entrypoint) = component_deno::resolve_entrypoint_with_meta(
                                &dep_path,
                                Some(&dep.organisation),
                                Some(&dep.name),
                                Some(&version.to_string()),
                            ) {
                                component_map.insert(component_id, (dep_path, entrypoint));
                            }
                        }
                    }
                }
            }
            let component_map = Arc::new(component_map);
            let project_path = project.path.clone();
            let project_name = project.name.clone();
            let project_org = project.organisation.clone();

            let resolver: component_deno::ComponentCallResolver = Box::new(move |component_id, method, spec, input, call_context| {
                let component_map = Arc::clone(&component_map);
                let project_path = project_path.clone();
                let project_name = project_name.clone();
                let project_org = project_org.clone();

                Box::pin(async move {
                    let (component_dir, entrypoint) = component_map
                        .get(&component_id)
                        .ok_or_else(|| anyhow::anyhow!("unknown component: {component_id}"))?;

                    // Use forwarded context if available, adding project defaults
                    let ctx = match call_context {
                        Some(mut ctx) => {
                            ctx.project = ctx.project.or(Some(project_name));
                            ctx.organisation = ctx.organisation.or(project_org);
                            ctx.work_dir = ctx.work_dir.or(Some(project_path.to_string_lossy().to_string()));
                            ctx
                        }
                        None => forest_sdk::CallContext {
                            project: Some(project_name),
                            organisation: project_org,
                            work_dir: Some(project_path.to_string_lossy().to_string()),
                            ..Default::default()
                        },
                    };

                    component_deno::invoke_deno_component(
                        component_dir, entrypoint, &method, &spec, &input, Some(&ctx), None,
                    ).await
                })
            });
            resolver
        };

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

            // Local deps use their declared path directly. Versioned
            // deps were downloaded into the per-component cache dir;
            // we read templates from there. Layout is identical to a
            // local component, so the rest of this function is
            // unchanged.
            let component_path = match &component.source {
                crate::models::ComponentSource::Local(path) => path.clone(),
                crate::models::ComponentSource::Versioned(version) => {
                    let cache_dir = state
                        .component_cache()
                        .versioned_component_dir(
                            &component.organisation,
                            &component.name,
                            version,
                        )
                        .await?;
                    if !cache_dir.exists() {
                        anyhow::bail!(
                            "component {}/{}@{} is not in the cache; run `forest deps` to download it first",
                            component.organisation, component.name, version,
                        );
                    }
                    cache_dir
                }
            };
            let component_path = &component_path;

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
                        &[("env", &deployment_item.env)],
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
                            Some(&call_resolver),
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

/// Apply a config override in the format "org/component.key.path=value".
///
/// The path before '=' is split:
///   - "org/component" → navigates to `other[org][component][config]`
///   - remaining dot-separated segments → nested map keys
///
/// Examples:
///   "kjuulh/service.tag=abc123"           → config.tag = "abc123"
///   "kjuulh/service.env_vars.LOG=debug"   → config.env_vars.LOG = "debug"
fn apply_config_override(root: &mut ProjectValue, kv: &str) -> anyhow::Result<()> {
    let (path, value) = kv
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("expected KEY=VALUE format"))?;

    // Split "org/component.key.subkey" into component ref + config path
    let (component_part, config_path) = path
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!("expected org/component.key format, got: {path}"))?;

    let (org, name) = component_part
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("expected org/component, got: {component_part}"))?;

    // Navigate to other[org][name][config]
    let ProjectValue::Map(root_map) = root else {
        anyhow::bail!("project config root is not a map");
    };

    let org_map = root_map
        .entry(org.to_string())
        .or_insert_with(|| ProjectValue::Map(Default::default()));
    let ProjectValue::Map(org_map) = org_map else {
        anyhow::bail!("organisation '{org}' config is not a map");
    };

    let comp_map = org_map
        .entry(name.to_string())
        .or_insert_with(|| ProjectValue::Map(Default::default()));
    let ProjectValue::Map(comp_map) = comp_map else {
        anyhow::bail!("component '{org}/{name}' config is not a map");
    };

    let config_map = comp_map
        .entry("config".to_string())
        .or_insert_with(|| ProjectValue::Map(Default::default()));
    let ProjectValue::Map(config_map) = config_map else {
        anyhow::bail!("component '{org}/{name}' config block is not a map");
    };

    // Navigate nested keys (e.g. "env_vars.LOG_LEVEL")
    let keys: Vec<&str> = config_path.split('.').collect();
    let mut current = config_map;

    for key in &keys[..keys.len() - 1] {
        let entry = current
            .entry(key.to_string())
            .or_insert_with(|| ProjectValue::Map(Default::default()));
        let ProjectValue::Map(next) = entry else {
            anyhow::bail!("config key '{key}' is not a map");
        };
        current = next;
    }

    let final_key = keys.last().unwrap().to_string();
    current.insert(final_key, ProjectValue::String(value.to_string()));

    Ok(())
}
