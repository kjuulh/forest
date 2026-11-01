use crate::{
    component_cache::ComponentCacheState,
    contracts::{self},
    models::ComponentSource,
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

            // Invoke commands/validate — try binary first, then deno
            let validate_result = if let Some(binary_path) =
                component_binary::resolve_binary_with_meta(
                    &path,
                    &dep.name,
                    Some(&dep.organisation),
                    Some(&dep.name),
                    version,
                ) {
                let input = serde_json::json!({});
                component_binary::invoke_component(
                    &binary_path,
                    "commands/validate",
                    &spec_json,
                    &input,
                )
                .await
            } else if crate::services::component_deno::is_deno_component_with_meta(
                &path,
                Some(&dep.organisation),
                Some(&dep.name),
                version,
            ) {
                if let Some(entrypoint) =
                    crate::services::component_deno::resolve_entrypoint_with_meta(
                        &path,
                        Some(&dep.organisation),
                        Some(&dep.name),
                        version,
                    )
                {
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
                    errors.push(format!("{dep_key}: no deno entrypoint found"));
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
                    let response = match serde_json::from_value::<ValidationResponse>(result) {
                        Ok(response) => response,
                        Err(_) => {
                            errors.push(format!(
                                "{dep_key}: validator did not return a boolean valid and string errors"
                            ));
                            continue;
                        }
                    };
                    let valid = response.valid && response.errors.is_empty();
                    let spec_errors = response.errors;

                    if valid {
                        eprintln!("  {} {}/{}  config valid", "✓", dep.organisation, dep.name);
                    } else {
                        if spec_errors.is_empty() {
                            errors.push(format!("{dep_key}: validator rejected config"));
                        }
                        for err in &spec_errors {
                            errors.push(format!("{}/{}: {}", dep.organisation, dep.name, err));
                        }
                        eprintln!(
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
                    eprintln!(
                        "  {} {}/{}  invalid config",
                        "✗", dep.organisation, dep.name
                    );
                    validated += 1;
                }
            }
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
