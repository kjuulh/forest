use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

/// Add a component dependency to the project.
///
/// Adds a dependency entry to forest.cue. Use --path for local development
/// or specify a version (default: latest from registry).
///
/// Examples:
///   forest add forest-contrib/kubernetes-service
///   forest add forest-contrib/kubernetes-service@0.2.0
///   forest add forest-contrib/kubernetes-service --path ../local-dev
#[derive(clap::Parser)]
pub struct AddCommand {
    /// Component to add (org/name or org/name@version)
    component: String,

    /// Use a local path instead of registry version
    #[arg(long)]
    path: Option<String>,
}

impl AddCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let (org_name, explicit_version) =
            if let Some((name, version)) = self.component.split_once('@') {
                (name.to_string(), Some(version.to_string()))
            } else {
                (self.component.clone(), None)
            };

        let (organisation, name) = org_name.split_once('/').ok_or_else(|| {
            anyhow::anyhow!(
                "component must be in org/name format (e.g., forest-contrib/kubernetes-service)"
            )
        })?;

        let dep_value = if let Some(path) = &self.path {
            format!("path: \"{}\"", path)
        } else {
            let version = if let Some(v) = explicit_version {
                v
            } else {
                let client = state.grpc_client();
                let component = client
                    .get_component(name, organisation)
                    .await
                    .context("failed to query registry")?
                    .ok_or_else(|| {
                        anyhow::anyhow!("component {organisation}/{name} not found in registry")
                    })?;
                component.version
            };
            format!("version: \"{}\"", version)
        };

        let forest_cue = find_forest_cue().await?;
        let content = tokio::fs::read_to_string(&forest_cue).await?;

        let dep_key = format!("\"{organisation}/{name}\"");
        if content.contains(&dep_key) {
            anyhow::bail!(
                "{organisation}/{name} is already in dependencies. \
                 Edit forest.cue to change the version or path."
            );
        }

        let new_content = insert_dependency(&content, &dep_key, &dep_value)?;

        tokio::fs::write(&forest_cue, &new_content).await?;

        let project_dir = forest_cue.parent().unwrap_or(std::path::Path::new("."));
        let cue_mod_dir = project_dir.join("cue.mod");
        let module_cue = cue_mod_dir.join("module.cue");

        if !module_cue.exists() {
            let project_name =
                extract_project_name(&new_content).unwrap_or_else(|| "my-project".to_string());
            let project_org =
                extract_project_org(&new_content).unwrap_or_else(|| "my-org".to_string());

            tokio::fs::create_dir_all(&cue_mod_dir).await?;
            let module_content = format!(
                "module: \"forest.sh/{project_org}/{project_name}@v0\"\n\
                 language: {{\n\tversion: \"v0.16.1\"\n}}\n\
                 source: {{\n\tkind: \"self\"\n}}\n\
                 deps: {{\n}}\n"
            );
            tokio::fs::write(&module_cue, &module_content).await?;
            eprintln!("Created cue.mod/module.cue");
        }

        if !self.path.is_some() {
            let version = dep_value
                .strip_prefix("version: \"")
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or("0.1.0");

            let module_content = tokio::fs::read_to_string(&module_cue).await?;
            let cue_dep_key = format!("\"forest.sh/{organisation}/{name}@v0\"");

            if !module_content.contains(&cue_dep_key) {
                let new_module = insert_cue_dep(&module_content, &cue_dep_key, version)?;
                tokio::fs::write(&module_cue, &new_module).await?;
                eprintln!("Added CUE module dependency to cue.mod/module.cue");
            }
        }

        let dep_display = if self.path.is_some() {
            format!(
                "{organisation}/{name} (local: {})",
                self.path.as_ref().unwrap()
            )
        } else {
            format!("{organisation}/{name} ({dep_value})")
        };

        eprintln!("Added {dep_display}");
        eprintln!();
        if self.path.is_none() {
            eprintln!("You can now import the component's types in your CUE files:");
            eprintln!(
                "  import k8s \"forest.sh/{organisation}/{name}@v0:{pkg}\"",
                pkg = name.replace('-', "_")
            );
        }

        Ok(())
    }
}

async fn find_forest_cue() -> anyhow::Result<std::path::PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        let candidate = dir.join("forest.cue");
        if candidate.exists() {
            return Ok(candidate);
        }
        let toml_candidate = dir.join("forest.toml");
        if toml_candidate.exists() {
            anyhow::bail!(
                "found forest.toml but `forest add` only supports forest.cue projects. \
                 Please migrate to forest.cue."
            );
        }
        if !dir.pop() {
            anyhow::bail!("no forest.cue found in current directory or parents");
        }
    }
}

fn insert_dependency(content: &str, dep_key: &str, dep_value: &str) -> anyhow::Result<String> {
    if let Some(deps_start) = content.find("dependencies:") {
        let after_deps = &content[deps_start..];
        if let Some(brace_offset) = after_deps.find('{') {
            let block_start = deps_start + brace_offset;

            let mut depth = 0;
            let mut block_end = None;
            for (i, ch) in content[block_start..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            block_end = Some(block_start + i);
                            break;
                        }
                    }
                    _ => {}
                }
            }

            if let Some(end) = block_end {
                let new_dep = format!("\t{dep_key}: {dep_value}\n");
                let mut result = String::with_capacity(content.len() + new_dep.len());
                result.push_str(&content[..end]);
                result.push_str(&new_dep);
                result.push_str(&content[end..]);
                return Ok(result);
            }
        }
    }

    let dep_block = format!("\ndependencies: {{\n\t{dep_key}: {dep_value}\n}}\n");
    Ok(format!("{content}{dep_block}"))
}

fn extract_project_name(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("name:") {
            return line
                .strip_prefix("name:")
                .map(|s| s.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn extract_project_org(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("organisation:") {
            return line
                .strip_prefix("organisation:")
                .map(|s| s.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn insert_cue_dep(content: &str, dep_key: &str, version: &str) -> anyhow::Result<String> {
    if let Some(deps_start) = content.find("deps:") {
        let after = &content[deps_start..];
        if let Some(brace_offset) = after.find('{') {
            let block_start = deps_start + brace_offset;
            let mut depth = 0;
            let mut block_end = None;
            for (i, ch) in content[block_start..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            block_end = Some(block_start + i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(end) = block_end {
                let new_dep = format!("\t{dep_key}: {{\n\t\tv: \"v{version}\"\n\t}}\n");
                let mut result = String::with_capacity(content.len() + new_dep.len());
                result.push_str(&content[..end]);
                result.push_str(&new_dep);
                result.push_str(&content[end..]);
                return Ok(result);
            }
        }
    }
    Ok(content.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_into_existing_deps() {
        let content = r#"project: {
	name: "my-api"
}

dependencies: {
	"forest-contrib/other": version: "1.0.0"
}
"#;
        let result = insert_dependency(
            content,
            "\"forest-contrib/kubernetes-service\"",
            "version: \"0.1.0\"",
        )
        .unwrap();

        assert!(result.contains("\"forest-contrib/kubernetes-service\": version: \"0.1.0\""));
        assert!(result.contains("\"forest-contrib/other\": version: \"1.0.0\""));
    }

    #[test]
    fn test_insert_with_no_deps_block() {
        let content = r#"project: {
	name: "my-api"
}
"#;
        let result = insert_dependency(
            content,
            "\"forest-contrib/kubernetes-service\"",
            "path: \"../local\"",
        )
        .unwrap();

        assert!(result.contains("dependencies: {"));
        assert!(result.contains("\"forest-contrib/kubernetes-service\": path: \"../local\""));
    }

    #[test]
    fn test_insert_local_path() {
        let content = r#"dependencies: {
}
"#;
        let result = insert_dependency(
            content,
            "\"my-org/my-comp\"",
            "path: \"../../components/my-comp\"",
        )
        .unwrap();

        assert!(result.contains("\"my-org/my-comp\": path: \"../../components/my-comp\""));
    }
}
