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

    #[serde(default)]
    pub templates: BTreeMap<String, RawSpecTemplate>,

    #[serde(default)]
    pub init: BTreeMap<String, Init>,
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
    pub require: bool,

    #[serde(default)]
    pub default: String,
}
