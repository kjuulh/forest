use crate::{
    contracts::{self},
    models::{ComponentSource, DependencyType},
    services::{component_binary, project::ProjectParserState},
    state::State,
};

/// Validate project configuration against component specs.
///
/// For each v2 component dependency, validates that the project's config
/// matches the component's spec schema. Also checks contract coverage:
/// which contracts are enabled and which components implement them.
///
/// Run from a project directory (where forest.cue lives).
#[derive(clap::Parser)]
pub struct ValidateCommand {}

impl ValidateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let project = state.project_parser().get_project().await?;

        // Derive available contracts from dependencies
        let enabled_contracts =
            contracts::EnabledContracts::from_project_dependencies(&project);
        if enabled_contracts.has_any() {
            println!("Contracts (from dependencies):");
            for topic in enabled_contracts.topics() {
                println!("  {} available", topic);
            }
            println!();
        }

        let mut errors = Vec::new();
        let mut validated = 0;
        let mut contract_implementations: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        for dep in &project.dependencies.dependencies {
            let path = match &dep.dependency_type {
                DependencyType::Local(path) => path.clone(),
                DependencyType::Versioned(_) => continue, // skip registry deps for now
            };

            if !component_binary::is_v2_component(&path) {
                continue;
            }

            // Skip contract-only dependencies (they define types, not services)
            let dep_key = format!("{}/{}", dep.organisation, dep.name);
            if contracts::is_contract(&dep_key) {
                continue;
            }

            // Build the spec from project config
            let comp_ref = crate::models::ComponentReference {
                organisation: dep.organisation.clone(),
                name: dep.name.clone(),
                source: ComponentSource::Local(path.clone()),
            };

            let spec_json = match project.get_component_config(&comp_ref) {
                Some(config) => serde_json::to_value(config).unwrap_or_default(),
                None => {
                    errors.push(format!(
                        "{}/{}: no config found in forest.cue",
                        dep.organisation, dep.name
                    ));
                    continue;
                }
            };

            // Check which contracts this component implements (before validation)
            let descriptor = component_binary::load_cached_descriptor(&path)
                .or_else(|| crate::services::component_deno::load_cached_descriptor(&path));
            if let Some(ref descriptor) = descriptor {
                let comp_contracts = contracts::component_contracts(descriptor);
                for topic in &comp_contracts {
                    contract_implementations
                        .entry(topic.clone())
                        .or_default()
                        .push(format!("{}/{}", dep.organisation, dep.name));
                }
            }

            // Invoke commands/validate — try binary first, then deno
            let validate_result = if let Some(binary_path) = component_binary::resolve_binary(&path, &dep.name) {
                let input = serde_json::json!({});
                component_binary::invoke_component(
                    &binary_path,
                    "commands/validate",
                    &spec_json,
                    &input,
                )
                .await
            } else if crate::services::component_deno::is_deno_component(&path) {
                if let Some(entrypoint) = crate::services::component_deno::resolve_entrypoint(&path) {
                    crate::services::component_deno::invoke_deno_component(
                        &path,
                        &entrypoint,
                        "commands/validate",
                        &spec_json,
                        &serde_json::json!({}),
                        None,
                        None,
                    )
                    .await
                } else {
                    continue;
                }
            } else {
                errors.push(format!(
                    "{}/{}: no binary or deno entrypoint found",
                    dep.organisation, dep.name
                ));
                continue;
            };

            match validate_result {
                Ok(result) => {
                    let valid = result
                        .get("valid")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    let spec_errors = result
                        .get("errors")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str())
                                .map(String::from)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();

                    if valid {
                        println!("  {} {}/{}  config valid", "✓", dep.organisation, dep.name);
                    } else {
                        for err in &spec_errors {
                            errors.push(format!("{}/{}: {}", dep.organisation, dep.name, err));
                        }
                        println!(
                            "  {} {}/{}  {} error(s)",
                            "✗",
                            dep.organisation,
                            dep.name,
                            spec_errors.len()
                        );
                    }
                    validated += 1;
                }
                Err(e) => {
                    let msg = e.to_string();
                    errors.push(format!("{}/{}: {}", dep.organisation, dep.name, msg));
                    println!("  {} {}/{}  invalid config", "✗", dep.organisation, dep.name);
                    validated += 1;
                }
            }
        }

        // Contract coverage check
        if enabled_contracts.has_any() {
            println!();
            println!("Contract coverage:");
            for topic in enabled_contracts.topics() {
                if let Some(implementors) = contract_implementations.get(topic) {
                    println!(
                        "  {} {}  implemented by: {}",
                        "✓",
                        topic,
                        implementors.join(", ")
                    );
                } else {
                    println!(
                        "  {} {}  no component implements this contract",
                        "!",
                        topic,
                    );
                }
            }
        }

        println!();
        if errors.is_empty() {
            println!("Validated {} component(s), all configs valid.", validated);
            Ok(())
        } else {
            println!("Validated {} component(s), {} error(s):", validated, errors.len());
            for err in &errors {
                println!("  - {err}");
            }
            anyhow::bail!("validation failed")
        }
    }
}
