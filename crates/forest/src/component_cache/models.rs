use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    path::PathBuf,
};

use anyhow::Context;

use crate::{
    models::{self, ComponentReference},
    services::component_parser::models::{
        RawComponent, RawComponentCommand, RawComponentDependency, RawComponentRequirement,
        RawComponentRequirementType,
    },
};

#[derive(Default, Clone)]
pub struct CacheComponents(pub Vec<CacheComponent>);
impl Deref for CacheComponents {
    type Target = Vec<CacheComponent>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for CacheComponents {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug, Clone)]
pub struct CacheComponent {
    pub name: String,
    pub namespace: String,
    pub version: semver::Version,

    pub dependencies: Vec<CacheComponentDependency>,

    pub requirements: Vec<CacheComponentRequirement>,

    pub commands: BTreeMap<String, CacheComponentCommand>,

    pub path: PathBuf,

    pub source: CacheComponentSource,
}
impl CacheComponent {
    pub(crate) fn component_ref(&self) -> ComponentReference {
        ComponentReference::new(
            &self.namespace,
            &self.name,
            match &self.source {
                CacheComponentSource::Versioned(version) => {
                    models::ComponentSource::Versioned(version.clone())
                }
                CacheComponentSource::Local(path) => models::ComponentSource::Local(path.clone()),
                CacheComponentSource::Unknown => todo!(),
            },
        )
    }
}

impl TryFrom<RawComponent> for CacheComponent {
    type Error = anyhow::Error;

    fn try_from(value: RawComponent) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.component_spec.component.name,
            namespace: value.component_spec.component.namespace,
            version: value
                .component_spec
                .component
                .version
                .parse()
                .context("semver")?,
            dependencies: value
                .component_spec
                .dependencies
                .into_iter()
                .map(|i| i.try_into())
                .collect::<anyhow::Result<Vec<_>>>()?,
            requirements: value
                .component_spec
                .requirements
                .into_iter()
                .map(|i| i.try_into())
                .collect::<anyhow::Result<Vec<_>>>()?,
            path: value.path,
            commands: value
                .component_spec
                .commands
                .into_iter()
                .map(|(command_name, command)| Ok((command_name, command.try_into()?)))
                .collect::<anyhow::Result<_>>()?,
            source: CacheComponentSource::Unknown,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CacheComponentDependency {
    pub name: String,
    pub namespace: String,
    pub version: semver::Version,
}

impl TryFrom<(String, RawComponentDependency)> for CacheComponentDependency {
    type Error = anyhow::Error;

    fn try_from((name, dependency): (String, RawComponentDependency)) -> Result<Self, Self::Error> {
        let (namespace, name) = match name.split_once("/") {
            Some((namespace, dep)) => (namespace, dep),
            None => ("forest", name.as_str()),
        };

        let version = match dependency {
            RawComponentDependency::String(version) => version,
            RawComponentDependency::Detailed(dep) => dep.version,
        };

        let version =
            semver::Version::parse(&version).context("failed to parse dependency version")?;

        Ok(Self {
            name: name.into(),
            namespace: namespace.into(),
            version,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CacheComponentRequirement {
    pub name: String,
    pub description: Option<String>,
    pub default: Option<String>,
    pub r#type: Option<CacheComponentRequirementType>,
}

#[derive(Debug, Clone)]
pub enum CacheComponentRequirementType {
    String,
}

impl TryFrom<(String, RawComponentRequirement)> for CacheComponentRequirement {
    type Error = anyhow::Error;

    fn try_from((entry, req): (String, RawComponentRequirement)) -> Result<Self, Self::Error> {
        Ok(Self {
            name: entry,
            description: req.description,
            default: req.default,
            r#type: req.r#type.map(|i| i.try_into()).transpose()?,
        })
    }
}

impl TryFrom<RawComponentRequirementType> for CacheComponentRequirementType {
    type Error = anyhow::Error;

    fn try_from(value: RawComponentRequirementType) -> Result<Self, Self::Error> {
        let val = match value {
            RawComponentRequirementType::String => Self::String,
        };

        Ok(val)
    }
}

#[derive(Debug, Clone)]
pub enum CacheComponentCommand {
    Inline(Vec<String>),
    Script(String),
}

impl TryFrom<RawComponentCommand> for CacheComponentCommand {
    type Error = anyhow::Error;

    fn try_from(value: RawComponentCommand) -> Result<Self, Self::Error> {
        match value {
            RawComponentCommand::Inline(items) => Ok(Self::Inline(items)),
            RawComponentCommand::Script(path) => Ok(Self::Script(path)),
            RawComponentCommand::InlineBash { bash } => Ok(Self::Inline(vec![bash])),
        }
    }
}

#[derive(Clone, Debug)]
pub enum CacheComponentSource {
    Versioned(semver::Version),
    Local(PathBuf),
    Unknown,
}
