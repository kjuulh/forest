use std::path::Path;

use crate::state::State;

use anyhow::Context;

pub mod models;
use models::*;

const COMPONENT_SPEC_FILE_NAME: &str = "forest.component.toml";
const COMPONENT_CUE_FILE_NAME: &str = "forest.component.cue";

#[derive(Clone)]
pub struct ComponentParser {}

impl ComponentParser {
    pub async fn parse(&self, path: &Path) -> anyhow::Result<RawComponent> {
        // Try v1 (TOML) first
        if let Some(component_spec) = get_component_spec_path(path).await? {
            return Ok(RawComponent {
                component_spec,
                path: path.into(),
            });
        }

        // Try v2 (CUE) — extract minimal metadata from forest.cue via cue export
        if path.join(COMPONENT_CUE_FILE_NAME).exists() {
            if let Some(component_spec) = get_component_spec_from_cue(path).await? {
                return Ok(RawComponent {
                    component_spec,
                    path: path.into(),
                });
            }
        }

        anyhow::bail!("failed to find component in path")
    }
}

/// Parse a v2 component's metadata from forest.cue via `cue export`.
async fn get_component_spec_from_cue(path: &Path) -> anyhow::Result<Option<RawComponentSpec>> {
    let forest_cue = path.join("forest.cue");
    let spec_cue = path.join("spec.cue");

    if !forest_cue.exists() {
        return Ok(None);
    }

    let mut cmd = tokio::process::Command::new("cue");
    cmd.arg("export")
        .arg(&forest_cue);
    if spec_cue.exists() {
        cmd.arg(&spec_cue);
    }
    cmd.arg("--out").arg("json");

    let output = cmd.output().await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::warn!("failed to parse v2 component CUE at {}: {}", path.display(), stderr);
        return Ok(None);
    }

    let doc: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    // Extract component metadata from forest.component section
    let component = doc
        .get("forest")
        .and_then(|f| f.get("component"));

    let (name, organisation, version) = match component {
        Some(comp) => {
            let name = comp.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
            let version = comp.get("version").and_then(|v| v.as_str()).unwrap_or("0.0.0");
            // Organisation comes from project section
            let org = doc
                .get("project")
                .and_then(|p| p.get("organisation"))
                .and_then(|v| v.as_str())
                .unwrap_or("forest");
            (name.to_string(), org.to_string(), version.to_string())
        }
        None => {
            tracing::warn!("v2 component at {} has no forest.component section", path.display());
            return Ok(None);
        }
    };

    Ok(Some(RawComponentSpec {
        component: RawSpecComponent {
            name,
            organisation,
            version,
        },
        // v2 components don't use TOML fields — commands come from the binary
        dependencies: Default::default(),
        templates: Default::default(),
        init: Default::default(),
        requirements: Default::default(),
        commands: Default::default(),
    }))
}

async fn get_component_spec_path(path: &Path) -> Result<Option<RawComponentSpec>, anyhow::Error> {
    let mut dir_entries = tokio::fs::read_dir(path)
        .await
        .context(format!("component path does not exist: {}", path.display()))?;
    let mut spec_file = None;

    while let Some(entry) = dir_entries.next_entry().await? {
        if entry.file_name() == COMPONENT_SPEC_FILE_NAME {
            spec_file = Some(entry.path());
            break;
        }
    }

    let Some(spec_file) = spec_file else {
        return Ok(None);
    };

    let component_file_content = tokio::fs::read_to_string(spec_file).await?;

    let component_spec: RawComponentSpec = toml::from_str(&component_file_content)?;

    Ok(Some(component_spec))
}

pub trait ComponentParserState {
    fn component_parser(&self) -> ComponentParser;
}

impl ComponentParserState for State {
    fn component_parser(&self) -> ComponentParser {
        ComponentParser {}
    }
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use crate::services::component_parser::{
        ComponentParser,
        models::RawSpecComponent,
    };

    #[tokio::test]
    async fn can_parse_template() -> anyhow::Result<()> {
        let parser = ComponentParser {};

        let raw_component = parser
            .parse(&PathBuf::from(
                "../../examples/rust-service-component/",
            ))
            .await?;

        assert_eq!(
            raw_component.path,
            PathBuf::from("../../examples/rust-service-component/")
        );
        assert_eq!(
            raw_component.component_spec.component,
            RawSpecComponent {
                name: "rust-service".into(),
                organisation: "forest-contrib".into(),
                version: "0.1.0".into()
            }
        );

        let commands = &raw_component.component_spec.commands;
        assert!(commands.contains_key("build"), "missing 'build' command");
        assert!(
            commands.contains_key("validate"),
            "missing 'validate' command"
        );
        assert!(commands.contains_key("test"), "missing 'test' command");
        assert!(
            commands.contains_key("docker-build"),
            "missing 'docker-build' command"
        );
        assert!(commands.contains_key("status"), "missing 'status' command");
        assert_eq!(commands.len(), 5);

        Ok(())
    }
}
