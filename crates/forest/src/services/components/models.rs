use uuid::Uuid;

use crate::{
    models::{Dependencies, Dependency, DependencyType},
    services::project::{self},
    user_config::{GlobalDependency, UserConfig},
};

#[derive(Clone, Debug, Default)]
pub struct Components {}

#[derive(Clone, Debug)]
pub struct UpstreamProjectDependency {
    pub id: Uuid,
    pub name: String,
    pub organisation: String,
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

impl TryFrom<(String, project::models::Dependency)> for Dependency {
    type Error = anyhow::Error;

    fn try_from(
        (name, dependency): (String, project::models::Dependency),
    ) -> Result<Self, Self::Error> {
        let (organisation, name) = match name.split_once("/") {
            Some((organisation, dep)) => (organisation, dep),
            None => ("forest", name.as_str()),
        };

        let dep = match dependency {
            project::Dependency::String(version) => {
                DependencyType::Versioned(version)
            }
            project::Dependency::Versioned(details) => {
                DependencyType::Versioned(details.version)
            }
            project::Dependency::Local(details) => DependencyType::Local(details.path),
        };

        Ok(Self {
            name: name.into(),
            organisation: organisation.into(),
            dependency_type: dep,
        })
    }
}

impl TryFrom<(String, GlobalDependency)> for Dependency {
    type Error = anyhow::Error;

    fn try_from((name, dependency): (String, GlobalDependency)) -> Result<Self, Self::Error> {
        let (organisation, name) = match name.split_once("/") {
            Some((organisation, dep)) => (organisation, dep),
            None => ("forest", name.as_str()),
        };

        Ok(Self {
            name: name.into(),
            organisation: organisation.into(),
            dependency_type: DependencyType::Versioned(dependency.version),
        })
    }
}
