use anyhow::Context;
use uuid::Uuid;

use crate::{
    models::{Dependencies, Dependency},
    services::project::{self, NonProject, ProjectDependency},
    user_config::{GlobalDependency, UserConfig},
};

#[derive(Clone, Debug, Default)]
pub struct Components {}

#[derive(Clone, Debug)]
pub struct UpstreamProjectDependency {
    pub id: Uuid,
    pub name: String,
    pub namespace: String,
    pub version: semver::Version,
}

impl TryFrom<UserConfig> for Dependencies {
    type Error = anyhow::Error;

    fn try_from(value: UserConfig) -> Result<Self, Self::Error> {
        (&value).try_into()
    }
}

impl TryFrom<&UserConfig> for Dependencies {
    type Error = anyhow::Error;

    fn try_from(value: &UserConfig) -> Result<Self, Self::Error> {
        let deps = value
            .dependencies
            .iter()
            .map(|(decl, dep)| (decl.clone(), dep.clone()))
            .map(|i| i.try_into())
            .collect::<anyhow::Result<Vec<_>>>();

        Ok(Self {
            dependencies: deps?,
        })
    }
}

impl TryFrom<(String, project::models::ProjectDependency)> for Dependency {
    type Error = anyhow::Error;

    fn try_from(
        (name, dependency): (String, project::models::ProjectDependency),
    ) -> Result<Self, Self::Error> {
        let (namespace, name) = match name.split_once("/") {
            Some((namespace, dep)) => (namespace, dep),
            None => ("non", name.as_str()),
        };

        let version = semver::Version::parse(&dependency.version)
            .context("failed to parse dependency version")?;

        Ok(Self {
            name: name.into(),
            namespace: namespace.into(),
            version,
        })
    }
}

impl TryFrom<(String, GlobalDependency)> for Dependency {
    type Error = anyhow::Error;

    fn try_from((name, dependency): (String, GlobalDependency)) -> Result<Self, Self::Error> {
        let (namespace, name) = match name.split_once("/") {
            Some((namespace, dep)) => (namespace, dep),
            None => ("non", name.as_str()),
        };

        let version = semver::Version::parse(&dependency.version)
            .context("failed to parse dependency version")?;

        Ok(Self {
            name: name.into(),
            namespace: namespace.into(),
            version,
        })
    }
}
