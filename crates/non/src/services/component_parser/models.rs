use std::{collections::BTreeMap, path::PathBuf};

use serde::Deserialize;

#[derive(Default, Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RawComponent {
    pub component_spec: RawComponentSpec,

    pub path: PathBuf,
}

#[derive(Default, Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RawComponentSpec {
    pub component: RawSpecComponent,

    pub dependencies: BTreeMap<String, RawComponentDependency>,

    #[serde(default)]
    pub templates: BTreeMap<String, RawSpecTemplate>,

    #[serde(default)]
    pub init: BTreeMap<String, Init>,

    #[serde(default)]
    pub requirements: BTreeMap<String, RawComponentRequirement>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub enum RawComponentDependency {
    String(String),
    Detailed(RawComponentDetailedDependency),
}

#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RawComponentDetailedDependency {
    pub version: String,
}

#[derive(Default, Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RawComponentRequirement {
    pub description: Option<String>,
    pub optional: bool,
    pub default: Option<String>,

    #[serde(rename = "type")]
    pub r#type: Option<RawComponentRequirementType>,
}

#[derive(Default, Clone, Debug, PartialEq, Eq, Deserialize)]
pub enum RawComponentRequirementType {
    #[default]
    String,
}

#[derive(Default, Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RawSpecComponent {
    pub name: String,
    pub namespace: String,
    pub version: String,
}

#[derive(Default, Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RawSpecTemplate {}

#[derive(Default, Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Init {
    #[serde(default)]
    pub input: BTreeMap<String, InitInput>,
}
#[derive(Default, Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct InitInput {
    #[serde(default)]
    pub required: bool,

    #[serde(default)]
    pub default: String,

    pub description: Option<String>,
}
