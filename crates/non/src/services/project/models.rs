use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NonProject {
    pub name: String,

    pub dependencies: BTreeMap<String, Dependency>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Dependency {
    String(String),
    Detailed(ProjectDependency),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProjectDependency {
    pub version: String,
}
