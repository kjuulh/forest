pub mod users;

use std::{collections::HashMap, fmt::Display, ops::Deref};

pub struct OrganisationName(String);
impl Deref for OrganisationName {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl From<String> for OrganisationName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<OrganisationName> for forest_grpc_interface::OrganisationRef {
    fn from(value: OrganisationName) -> Self {
        Self {
            organisation: value.0,
        }
    }
}
impl From<forest_grpc_interface::OrganisationRef> for OrganisationName {
    fn from(value: forest_grpc_interface::OrganisationRef) -> Self {
        Self(value.organisation)
    }
}

pub struct ProjectName(String);
impl Deref for ProjectName {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl From<String> for ProjectName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

pub struct Project {
    pub organisation: OrganisationName,
    pub name: ProjectName,
}

impl From<forest_grpc_interface::Project> for Project {
    fn from(value: forest_grpc_interface::Project) -> Self {
        Self {
            organisation: value.organisation.into(),
            name: value.project.into(),
        }
    }
}
impl From<Project> for forest_grpc_interface::Project {
    fn from(value: Project) -> Self {
        Self {
            organisation: value.organisation.to_string(),
            project: value.name.to_string(),
        }
    }
}

pub struct Destination {
    pub organisation: String,
    pub name: String,
    pub environment: String,
    pub metadata: HashMap<String, String>,

    pub destination_type: DestinationType,
}

impl Destination {
    pub fn new(
        organisation: &str,
        name: &str,
        environment: &str,
        metadata: HashMap<String, String>,
        destination_type: DestinationType,
    ) -> Self {
        Self {
            organisation: organisation.into(),
            name: name.into(),
            environment: environment.into(),
            metadata,
            destination_type,
        }
    }
}

impl Display for Destination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl From<Destination> for forest_grpc_interface::Destination {
    fn from(value: Destination) -> Self {
        Self {
            organisation: value.organisation,
            name: value.name,
            environment: value.environment,
            r#type: Some(value.destination_type.into()),
            metadata: value.metadata,
        }
    }
}

pub struct DestinationType {
    pub organisation: String,
    pub name: String,
    pub version: usize,
}

impl Display for DestinationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}@{}", self.organisation, self.name, self.version)
    }
}

impl From<DestinationType> for forest_grpc_interface::DestinationType {
    fn from(value: DestinationType) -> Self {
        Self {
            organisation: value.organisation,
            name: value.name,
            version: value.version as u64,
        }
    }
}

impl From<forest_grpc_interface::DestinationType> for DestinationType {
    fn from(value: forest_grpc_interface::DestinationType) -> Self {
        Self {
            organisation: value.organisation,
            name: value.name,
            version: value.version as usize,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseStatus {
    Staged,
    Running,
    Success,
    Failure,
}

impl ReleaseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReleaseStatus::Staged => "STAGED",
            ReleaseStatus::Running => "RUNNING",
            ReleaseStatus::Success => "SUCCESS",
            ReleaseStatus::Failure => "FAILURE",
        }
    }

    pub fn is_finalized(&self) -> bool {
        matches!(self, ReleaseStatus::Success | ReleaseStatus::Failure)
    }

    pub fn is_running(&self) -> bool {
        matches!(self, ReleaseStatus::Running)
    }

    pub fn is_success(&self) -> bool {
        matches!(self, ReleaseStatus::Success)
    }

    pub fn is_failure(&self) -> bool {
        matches!(self, ReleaseStatus::Failure)
    }
}

impl Display for ReleaseStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for ReleaseStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "STAGED" => Ok(ReleaseStatus::Staged),
            "RUNNING" => Ok(ReleaseStatus::Running),
            "SUCCESS" => Ok(ReleaseStatus::Success),
            "FAILURE" => Ok(ReleaseStatus::Failure),
            _ => Err(format!("unknown release status: {}", s)),
        }
    }
}

impl From<ReleaseStatus> for String {
    fn from(value: ReleaseStatus) -> Self {
        value.as_str().to_string()
    }
}
