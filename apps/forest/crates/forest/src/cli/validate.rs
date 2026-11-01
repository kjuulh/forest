use crate::{
    component_cache::ComponentCacheState,
    contracts::{self},
    models::ComponentSource,
    services::{component_binary, project::ProjectParserState},
    state::State,
};

/// Validate project configuration against component specs.
///
/// Validates local and registry component configs against CUE #Spec, merging
/// environment overrides as release preparation does. Custom commands/validate
/// runs only when declared. Also reports available contracts and implementations.
///
/// Run from a project directory (where forest.cue lives).
#[derive(clap::Parser)]
pub struct ValidateCommand {}

impl ValidateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        // TASKS/028: run the publish-readiness preflight first. Until
        // this was added, `forest validate` reported "Validated 0
        // component(s)" on projects whose names disagreed across files —
        // a silent pass through real configuration errors. Preflight
        // failures stop validation here; if you can't publish, the
        // dependency-config check is moot.
        let current_dir = std::env::current_dir()?;
        // build_context re-evaluates cue, which is the same work the
        // dependency-config validation below relies on the project
        // parser to have already done. The duplicate eval is cheap and
        // keeps preflight standalone-runnable.
        match crate::services::preflight::build_context(&current_dir).await {
            Ok(ctx) => {
                let checks = crate::services::preflight::standard_checks();
                match crate::services::preflight::run_checks(&ctx, &checks) {
                    Ok(()) => {
                        eprintln!(
                            "Preflight: {} check(s) passed for {}/{}",
                            checks.len(),
                            ctx.organisation,
                            ctx.component_name,
                        );
                    }
                    Err(failures) => {
                        eprint!("{}", crate::services::preflight::render_failures(&failures));
                        anyhow::bail!("preflight failed");
                    }
                }
            }
            Err(e) => {
                // If we can't even build the context (e.g. there's no
                // forest.cue here), fall through to the dependency-config
                // path which has its own diagnostics. Don't bail — the
                // user may be invoking `validate` from a non-project dir
                // intentionally, in which case the dep path is the only
                // useful work to do.
                tracing::debug!("preflight context unavailable: {e:#}");
            }
        }

        let project = state.project_parser().get_project().await?;

        // Derive available contracts from dependencies
        let enabled_contracts = contracts::EnabledContracts::from_project_dependencies(&project);
        if enabled_contracts.has_any() {
            eprintln!("Contracts (from dependencies):");
            for topic in enabled_contracts.topics() {
                eprintln!("  {} available", topic);
            }
            eprintln!();
        }

        let mut errors = Vec::new();
        let mut validated = 0;
        let mut contract_implementations: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        for comp_ref in project.dependencies.get_components() {
            let dep_key = format!("{}/{}", comp_ref.organisation, comp_ref.name);
            if contracts::is_contract(&dep_key) {
                continue;
            }
            // Peer dependencies without a usage block have no project config to validate.
            if !project.has_component_usage(&comp_ref.organisation, &comp_ref.name) {
                continue;
            }
            let (path, version) = match &comp_ref.source {
                ComponentSource::Local(path) => (path.clone(), None),
                ComponentSource::Versioned(version) => (
                    state
                        .component_cache()
                        .versioned_component_dir(&comp_ref.organisation, &comp_ref.name, version)
                        .await?,
                    Some(version.as_str()),
                ),
            };
            let dep = &comp_ref;
            if !component_binary::is_v2_component(&path) {
                errors.push(format!(
                    "{dep_key}: no v2 component validator found at {}",
                    path.display()
                ));
                continue;
            }

            let configs = match effective_configs(&project, &comp_ref) {
                Ok(configs) => configs,
                Err(e) => {
                    errors.push(format!("{dep_key}: {e}"));
                    continue;
                }
            };
            let mut schema_files = vec!["./forest.component.cue"];
            if path.join("spec.cue").exists() {
                schema_files.push("./spec.cue");
            }
            let schema =
                match super::components::generate::run_cue_def_openapi_in_dir(&schema_files, &path)
                    .await
                    .and_then(|json| {
                        serde_json::from_str::<serde_json::Value>(&json).map_err(Into::into)
                    }) {
                    Ok(schema) => schema,
                    Err(e) => {
                        errors.push(format!("{dep_key}: cannot load component schema: {e}"));
                        continue;
                    }
                };
            let custom_validation = schema
                .pointer("/components/schemas/Commands")
                .is_some_and(|commands| declares_validate(commands, &schema));

            // Check which contracts this component implements (before validation)
            let descriptor = component_binary::load_cached_descriptor_with_meta(
                &path,
                Some(&dep.organisation),
                Some(&dep.name),
                version,
            )
            .or_else(|| {
                crate::services::component_deno::load_cached_descriptor_with_meta(
                    &path,
                    Some(&dep.organisation),
                    Some(&dep.name),
                    version,
                )
            });
            if let Some(ref descriptor) = descriptor {
                let comp_contracts = contracts::component_contracts(descriptor);
                for topic in &comp_contracts {
                    contract_implementations
                        .entry(topic.clone())
                        .or_default()
                        .push(format!("{}/{}", dep.organisation, dep.name));
                }
            }

            let mut component_valid = true;
            for (scope, config) in configs {
                let result = validate_config(
                    &path,
                    &schema_files,
                    dep,
                    version,
                    custom_validation,
                    &serde_json::to_value(config)?,
                )
                .await;
                if let Err(e) = result {
                    errors.push(format!("{dep_key} ({scope}): {e}"));
                    component_valid = false;
                }
            }
            if component_valid {
                eprintln!("  ✓ {dep_key}  config valid");
            } else {
                eprintln!("  ✗ {dep_key}  invalid config");
            }
            validated += 1;
        }

        // Contract coverage check
        if enabled_contracts.has_any() {
            eprintln!();
            eprintln!("Contract coverage:");
            for topic in enabled_contracts.topics() {
                if let Some(implementors) = contract_implementations.get(topic) {
                    eprintln!(
                        "  {} {}  implemented by: {}",
                        "✓",
                        topic,
                        implementors.join(", ")
                    );
                } else {
                    eprintln!("  {} {}  no component implements this contract", "!", topic,);
                }
            }
        }

        eprintln!();
        if errors.is_empty() {
            if validated == 0 {
                eprintln!("No component configs to validate.");
            } else {
                eprintln!("Validated {} component(s), all configs valid.", validated);
            }
            Ok(())
        } else {
            eprintln!(
                "Validated {} component(s), {} error(s):",
                validated,
                errors.len()
            );
            for err in &errors {
                eprintln!("  - {err}");
            }
            anyhow::bail!("validation failed")
        }
    }
}

#[derive(serde::Deserialize)]
struct ValidationResponse {
    valid: bool,
    #[serde(default)]
    errors: Vec<String>,
}

fn declares_validate(commands: &serde_json::Value, document: &serde_json::Value) -> bool {
    if commands.pointer("/properties/validate").is_some() {
        return true;
    }
    if let Some(reference) = commands.get("$ref").and_then(|value| value.as_str())
        && let Some(target) = reference
            .strip_prefix('#')
            .and_then(|path| document.pointer(path))
    {
        return declares_validate(target, document);
    }
    commands
        .get("allOf")
        .and_then(|value| value.as_array())
        .is_some_and(|schemas| {
            schemas
                .iter()
                .any(|schema| declares_validate(schema, document))
        })
}

fn effective_configs(
    project: &crate::models::Project,
    component: &crate::models::ComponentReference,
) -> anyhow::Result<Vec<(String, crate::models::ProjectValue)>> {
    use crate::models::ProjectValue;
    let Some(ProjectValue::Map(usage)) = project.other.get_from_component(component) else {
        anyhow::bail!("component usage must be a map");
    };
    let base = usage.get("config");
    if let Some(envs) = usage.get("env") {
        let ProjectValue::Map(envs) = envs else {
            anyhow::bail!("env must be a map");
        };
        if !envs.is_empty() {
            let mut configs = Vec::with_capacity(envs.len());
            for (name, env) in envs {
                let ProjectValue::Map(env) = env else {
                    anyhow::bail!("env {name} must be a map");
                };
                let config = match (base, env.get("config")) {
                    (Some(base), Some(patch)) => {
                        super::release::prepare::merge_config(base.clone(), patch.clone())
                    }
                    (Some(config), None) | (None, Some(config)) => config.clone(),
                    (None, None) => anyhow::bail!("env {name}: no config found in forest.cue"),
                };
                configs.push((format!("env {name}"), config));
            }
            configs.sort_by(|a, b| a.0.cmp(&b.0));
            return Ok(configs);
        }
    }
    Ok(vec![(
        "config".to_string(),
        base.cloned()
            .ok_or_else(|| anyhow::anyhow!("no config found in forest.cue"))?,
    )])
}

async fn validate_config(
    path: &std::path::Path,
    schema_files: &[&str],
    component: &crate::models::ComponentReference,
    version: Option<&str>,
    custom_validation: bool,
    spec: &serde_json::Value,
) -> anyhow::Result<()> {
    let mut config = tempfile::Builder::new().suffix(".json").tempfile()?;
    serde_json::to_writer(config.as_file_mut(), spec)?;
    let output = crate::tools::cue::output(|| {
        let mut cmd = tokio::process::Command::new("cue");
        cmd.current_dir(path)
            .args(["vet", "-c", "-d", "#Spec"])
            .args(schema_files)
            .arg(config.path());
        cmd
    })
    .await?;
    if !output.status.success() {
        // CUE diagnostics can include supplied values, which may be secrets.
        anyhow::bail!(
            "config does not conform to component #Spec (cue vet {})",
            output.status
        );
    }
    if !custom_validation {
        return Ok(());
    }
    let result = if let Some(binary) = component_binary::resolve_binary_with_meta(
        path,
        &component.name,
        Some(&component.organisation),
        Some(&component.name),
        version,
    ) {
        component_binary::invoke_component(
            &binary,
            "commands/validate",
            spec,
            &serde_json::json!({}),
        )
        .await?
    } else if let Some(entrypoint) = crate::services::component_deno::resolve_entrypoint_with_meta(
        path,
        Some(&component.organisation),
        Some(&component.name),
        version,
    ) {
        crate::services::component_deno::invoke_deno_component(
            path,
            &entrypoint,
            "commands/validate",
            spec,
            &serde_json::json!({}),
            None,
            None,
        )
        .await?
    } else {
        anyhow::bail!("declared commands/validate has no binary or deno entrypoint");
    };
    let response: ValidationResponse = serde_json::from_value(result).map_err(|_| {
        anyhow::anyhow!("validator did not return a boolean valid and string errors")
    })?;
    if !response.valid || !response.errors.is_empty() {
        if response.errors.is_empty() {
            anyhow::bail!("validator rejected config");
        }
        anyhow::bail!("{}", response.errors.join("; "));
    }
    Ok(())
}
