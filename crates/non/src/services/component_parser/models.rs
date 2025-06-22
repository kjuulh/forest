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

    pub templates: BTreeMap<String, RawSpecTemplate>,
}
#[derive(Default, Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RawSpecComponent {
    pub name: String,
    pub namespace: String,
    pub version: String,
}

#[derive(Default, Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RawSpecTemplate {}
