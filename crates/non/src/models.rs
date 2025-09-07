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
    pub version: DependencyVersion,
}

type DependencyName = String;
type DependencyNamespace = String;
type DependencyVersion = semver::Version;

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
