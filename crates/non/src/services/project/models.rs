use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Project {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ProjectDependency {
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ProjectFile {
    pub project: Project,
    pub dependencies: BTreeMap<String, ProjectDependency>,
}
