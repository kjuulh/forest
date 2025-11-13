use std::path::Path;

use crate::state::State;

use anyhow::Context;

pub mod models;
use models::*;

const COMPONENT_SPEC_FILE_NAME: &str = "non.component.toml";

pub struct ComponentParser {}

impl ComponentParser {
    pub async fn parse(&self, path: &Path) -> anyhow::Result<RawComponent> {
        let component_spec = get_component_spec_path(path)
            .await?
            .ok_or(anyhow::anyhow!("failed to find component in path"))?;

        Ok(RawComponent {
            component_spec,
            path: path.into(),
        })
    }
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
    use std::{collections::BTreeMap, path::PathBuf};

    use crate::services::component_parser::{
        ComponentParser,
        models::{RawComponent, RawComponentSpec, RawSpecComponent, RawSpecTemplate},
    };

    #[tokio::test]
    async fn can_parse_template() -> anyhow::Result<()> {
        let parser = ComponentParser {};

        let raw_component = parser
            .parse(&PathBuf::from("../../examples/my_component/"))
            .await?;

        assert_eq!(
            RawComponent {
                path: PathBuf::from("../../examples/my_component/"),
                component_spec: RawComponentSpec {
                    component: RawSpecComponent {
                        name: "my_component".into(),
                        namespace: "non".into(),
                        version: "0.0.2".into()
                    },
                    templates: BTreeMap::from([("rust_service".to_string(), RawSpecTemplate {})]),
                    init: BTreeMap::default(),
                    requirements: BTreeMap::default(),
                    dependencies: BTreeMap::default(),
                    commands: BTreeMap::new(),
                }
            },
            raw_component
        );

        Ok(())
    }
}
