use std::{collections::HashMap, fmt::Display, ops::Deref};

pub struct Namespace(String);
impl Deref for Namespace {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl From<String> for Namespace {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<Namespace> for non_grpc_interface::Namespace {
    fn from(value: Namespace) -> Self {
        Self { namespace: value.0 }
    }
}
impl From<non_grpc_interface::Namespace> for Namespace {
    fn from(value: non_grpc_interface::Namespace) -> Self {
        Self(value.namespace)
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
    pub namespace: Namespace,
    pub name: ProjectName,
}

impl From<non_grpc_interface::Project> for Project {
    fn from(value: non_grpc_interface::Project) -> Self {
        Self {
            namespace: value.namespace.into(),
            name: value.project.into(),
        }
    }
}
impl From<Project> for non_grpc_interface::Project {
    fn from(value: Project) -> Self {
        Self {
            namespace: value.namespace.to_string(),
            project: value.name.to_string(),
        }
    }
}

pub struct Destination {
    pub name: String,
    pub environment: String,
    pub metadata: HashMap<String, String>,

    pub destination_type: DestinationType,
}

impl Destination {
    pub fn new(
        name: &str,
        environment: &str,
        metadata: HashMap<String, String>,
        destination_type: DestinationType,
    ) -> Self {
        Self {
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

impl From<Destination> for non_grpc_interface::Destination {
    fn from(value: Destination) -> Self {
        Self {
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

impl From<DestinationType> for non_grpc_interface::DestinationType {
    fn from(value: DestinationType) -> Self {
        Self {
            organisation: value.organisation,
            name: value.name,
            version: value.version as u64,
        }
    }
}

impl From<non_grpc_interface::DestinationType> for DestinationType {
    fn from(value: non_grpc_interface::DestinationType) -> Self {
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
    Success,
    Failure,
}

impl ReleaseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReleaseStatus::Staged => "STAGED",
            ReleaseStatus::Success => "SUCCESS",
            ReleaseStatus::Failure => "FAILURE",
        }
    }

    pub fn is_finalized(&self) -> bool {
        matches!(self, ReleaseStatus::Success | ReleaseStatus::Failure)
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
