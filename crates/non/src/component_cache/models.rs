use std::collections::BTreeMap;

use crate::services::components::models::ProjectDependency;

#[derive(Clone, Debug, Default)]
pub struct LocalComponents {
    pub components: Vec<LocalComponent>,
}
impl LocalComponents {
    pub(crate) fn get(
        &self,
        dependencies: Vec<ProjectDependency>,
    ) -> anyhow::Result<LocalComponents> {
        let mut components = Vec::new();

        for dep in &dependencies {
            let component = self
                .components
                .iter()
                .find(|c| {
                    dep.name == c.name
                        && dep.namespace == c.namespace
                        && dep.version.to_string() == c.version
                })
                .ok_or(anyhow::anyhow!("failed to find local component"))?;

            components.push(component.clone());
        }

        Ok(LocalComponents { components })
    }

    pub fn get_init(&self) -> BTreeMap<String, LocalComponent> {
        self.components
            .iter()
            .cloned()
            .flat_map(|i| {
                i.init
                    .keys()
                    .map(|k| (format!("{}:{}:{}", i.namespace, i.name, k), i.clone()))
                    .collect::<BTreeMap<String, LocalComponent>>()
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalComponent {
    pub name: String,
    pub namespace: String,
    pub version: String,

    pub init: BTreeMap<String, Init>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Init {
    pub require: bool,
    pub default: String,
}
