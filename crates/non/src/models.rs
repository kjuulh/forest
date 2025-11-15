use std::{collections::BTreeMap, fmt::Display, path::PathBuf};

use anyhow::Context;

pub mod artifacts {
    pub type ArtifactID = uuid::Uuid;
}

pub mod source {
    #[derive(Clone)]
    pub struct Source {
        pub username: Option<String>,
        pub email: Option<String>,
    }
}

pub mod context {
    #[derive(Clone)]
    pub struct ArtifactContext {
        pub title: String,
        pub description: Option<String>,
        pub web: Option<String>,
    }
}

pub mod release_annotation {
    use std::collections::HashMap;

    use uuid::Uuid;

    use crate::models::{context::ArtifactContext, source::Source};

    pub struct ReleaseAnnotation {
        pub id: Uuid,
        pub artifact_id: Uuid,
        pub slug: String,
        pub metadata: HashMap<String, String>,
        pub source: Source,
        pub context: ArtifactContext,
    }
}

pub mod project {
    #[derive(Clone)]
    pub struct Project {
        pub namespace: String,
        pub project: String,
    }
}

pub mod reference {
    #[derive(Clone)]
    pub struct Reference {
        pub commit_sha: String,
        pub commit_branch: Option<String>,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub name: String,
    pub dependencies: Dependencies,

    pub commands: BTreeMap<CommandName, Command>,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, PartialOrd, Ord, Eq)]
pub enum CommandName {
    Component {
        namespace: Option<String>,
        name: String,
        source: CommandSource,
        command_name: String,
    },
    Project {
        command_name: String,
    },
}
impl CommandName {
    pub(crate) fn command_name(&self) -> &str {
        match self {
            CommandName::Component { command_name, .. } => &command_name,
            CommandName::Project { command_name } => &command_name,
        }
    }

    pub fn to_fqn(&self) -> String {
        match self {
            CommandName::Component {
                namespace,
                name,
                source,
                command_name,
            } => {
                let src = match source {
                    CommandSource::Local(path) => format!("#{}", path.to_string_lossy()),
                    CommandSource::Versioned(version) => format!("@{}", version.to_string()),
                };
                format!(
                    "{}/{}{src}:{command_name}",
                    namespace.as_ref().unwrap_or(&"non".to_string()),
                    name,
                )
            }
            CommandName::Project { command_name } => format!(":{}", command_name),
        }
    }

    pub fn to_component(&self) -> Option<String> {
        match self {
            CommandName::Component {
                namespace,
                name,
                source,
                ..
            } => {
                let src = match source {
                    CommandSource::Local(path) => format!("#{}", path.to_string_lossy()),
                    CommandSource::Versioned(version) => format!("@{}", version.to_string()),
                };
                Some(format!(
                    "{}/{}{src}",
                    namespace.as_ref().unwrap_or(&"non".to_string()),
                    name,
                ))
            }
            CommandName::Project { .. } => None,
        }
    }
}

impl Display for CommandName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandName::Component {
                namespace,
                name,
                source,
                command_name,
            } => {
                write!(
                    f,
                    "{}{}{}:{}",
                    match &namespace {
                        Some(item) => format!("{item}/"),
                        None => "".to_string(),
                    },
                    &name,
                    {
                        match &source {
                            CommandSource::Local(path) => format!("#{}", path.to_string_lossy()),
                            CommandSource::Versioned(version) => {
                                format!("@{}", version.to_string())
                            }
                        }
                    },
                    command_name
                )
            }
            CommandName::Project { command_name } => f.write_str(&command_name),
        }
    }
}

#[derive(Clone, Debug, PartialEq, PartialOrd, Ord, Eq)]
pub struct ComponentReference {
    pub namespace: String,
    pub name: String,
    pub source: ComponentSource,
}
impl ComponentReference {
    pub(crate) fn new(namespace: &str, name: &str, source: ComponentSource) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
            source: source.into(),
        }
    }
}

impl Display for ComponentReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}{}",
            self.namespace,
            self.name,
            match &self.source {
                ComponentSource::Local(path) => format!("#{}", path.to_string_lossy()),
                ComponentSource::Versioned(version) => format!("@{version}"),
            }
        )
    }
}

impl TryFrom<String> for ComponentReference {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.as_str().try_into()
    }
}

impl TryFrom<&str> for ComponentReference {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let (namespace, rest) = match value.split_once("/") {
            Some((namespace, rest)) => (namespace, rest),
            None => ("non", value),
        };

        let (name, source) = if let Some((name, rest)) = rest.split_once("@") {
            (
                name,
                ComponentSource::Versioned(rest.parse().context("parse version")?),
            )
        } else if let Some((name, rest)) = rest.split_once("#") {
            (name, ComponentSource::Local(PathBuf::from(rest)))
        } else {
            anyhow::bail!("component reference must include either a version or path")
        };

        Ok(Self {
            namespace: namespace.into(),
            name: name.into(),
            source,
        })
    }
}

#[derive(Clone, Debug, PartialEq, PartialOrd, Ord, Eq)]
pub enum ComponentSource {
    Local(PathBuf),
    Versioned(semver::Version),
}

#[derive(Clone, Debug, PartialEq, PartialOrd, Ord, Eq)]
pub enum CommandSource {
    Local(PathBuf),
    Versioned(semver::Version),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Script(String),
    Inline(Vec<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dependencies {
    pub dependencies: Vec<Dependency>,
}

impl Dependencies {
    pub fn diff(&self, right: Vec<impl Into<Dependency>>) -> (Dependencies, Dependencies) {
        let components: Vec<Dependency> = right.into_iter().map(|c| c.into()).collect::<Vec<_>>();

        let right_components = components
            .iter()
            .filter(|r| !self.dependencies.iter().any(|l| l == *r))
            .cloned()
            .collect::<Vec<_>>();

        let left_components = self
            .dependencies
            .iter()
            .filter(|r| !right_components.iter().any(|l| l == *r))
            .cloned()
            .collect::<Vec<_>>();

        (
            Dependencies {
                dependencies: left_components,
            },
            Dependencies {
                dependencies: right_components,
            },
        )
    }

    pub fn merge(&mut self, dependencies: &mut Dependencies) {
        self.dependencies.append(&mut dependencies.dependencies);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dependency {
    pub name: DependencyName,
    pub namespace: DependencyNamespace,

    pub dependency_type: DependencyType,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DependencyType {
    Versioned(DependencyVersion),
    Local(DependencyPath),
}

type DependencyName = String;
type DependencyNamespace = String;
type DependencyVersion = semver::Version;
type DependencyPath = PathBuf;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Requirements {
    pub requirements: Vec<Requirement>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Requirement {
    pub name: String,
    pub optional: bool,
    pub default: Option<RequirementValue>,
    pub value: Option<RequirementValue>,
}

impl Requirement {
    pub fn get_value(&self) -> anyhow::Result<Option<&RequirementValue>> {
        let Some(value) = &self.value else {
            if self.optional {
                match &self.default {
                    Some(val) => return Ok(Some(val)),
                    None => return Ok(None),
                }
            }

            match &self.default {
                Some(val) => return Ok(Some(val)),
                None => anyhow::bail!("value: {} is required", self.name),
            }
        };

        Ok(Some(value))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RequirementValue {
    String(String),
}
