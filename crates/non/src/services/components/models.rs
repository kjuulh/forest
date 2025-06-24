use anyhow::Context;
use uuid::Uuid;

use crate::services::project::{self, Project};

#[derive(Clone, Debug, Default)]
pub struct Components {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectDependency {
    pub name: String,
    pub namespace: String,
    pub version: semver::Version,
}

#[derive(Clone, Debug)]
pub struct ProjectDependencies {
    pub(super) dependencies: Vec<ProjectDependency>,
}

impl ProjectDependencies {
    pub fn diff_right(&self, right: Vec<impl Into<ProjectDependency>>) -> ProjectDependencies {
        let components: Vec<ProjectDependency> =
            right.into_iter().map(|c| c.into()).collect::<Vec<_>>();

        let right_components = components
            .iter()
            .filter(|r| self.dependencies.iter().any(|l| l == *r))
            .cloned()
            .collect::<Vec<_>>();

        ProjectDependencies {
            dependencies: right_components,
        }
    }
}

#[derive(Clone, Debug)]
pub struct UpstreamProjectDependency {
    pub id: Uuid,
    pub name: String,
    pub namespace: String,
    pub version: semver::Version,
}

impl TryFrom<Project> for ProjectDependencies {
    type Error = anyhow::Error;

    fn try_from(value: Project) -> Result<Self, Self::Error> {
        (&value).try_into()
    }
}

impl TryFrom<&Project> for ProjectDependencies {
    type Error = anyhow::Error;

    fn try_from(value: &Project) -> Result<Self, Self::Error> {
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

impl TryFrom<(String, project::models::ProjectDependency)> for ProjectDependency {
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
