use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NonProject {
    pub project: Project,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Project {
    pub name: String,
    pub dependencies: BTreeMap<String, Dependency>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Dependency {
    String(String),
    Versioned(VersionedDependency),
    Local(LocalDependency),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct VersionedDependency {
    pub version: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LocalDependency {
    pub path: PathBuf,
}
