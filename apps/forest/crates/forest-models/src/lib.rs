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

pub struct MetadataFieldSchema {
    pub name: String,
    pub label: String,
    pub description: String,
    pub required: bool,
    pub field_type: String,
    pub default_value: String,
}

pub struct DestinationType {
    pub organisation: String,
    pub name: String,
    pub version: usize,
    pub description: String,
    pub fields: Vec<MetadataFieldSchema>,
}

impl Display for DestinationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}@{}", self.organisation, self.name, self.version)
    }
}

impl From<MetadataFieldSchema> for forest_grpc_interface::MetadataFieldSchema {
    fn from(value: MetadataFieldSchema) -> Self {
        Self {
            name: value.name,
            label: value.label,
            description: value.description,
            required: value.required,
            field_type: value.field_type,
            default_value: value.default_value,
        }
    }
}

impl From<DestinationType> for forest_grpc_interface::DestinationType {
    fn from(value: DestinationType) -> Self {
        Self {
            organisation: value.organisation,
            name: value.name,
            version: value.version as u64,
            description: value.description,
            fields: value.fields.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<forest_grpc_interface::DestinationType> for DestinationType {
    fn from(value: forest_grpc_interface::DestinationType) -> Self {
        Self {
            organisation: value.organisation,
            name: value.name,
            version: value.version as usize,
            description: value.description,
            fields: vec![],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseStatus {
    Queued,
    Assigned,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

impl ReleaseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReleaseStatus::Queued => "QUEUED",
            ReleaseStatus::Assigned => "ASSIGNED",
            ReleaseStatus::Running => "RUNNING",
            ReleaseStatus::Succeeded => "SUCCEEDED",
            ReleaseStatus::Failed => "FAILED",
            ReleaseStatus::Cancelled => "CANCELLED",
            ReleaseStatus::TimedOut => "TIMED_OUT",
        }
    }

    pub fn is_finalized(&self) -> bool {
        matches!(
            self,
            ReleaseStatus::Succeeded
                | ReleaseStatus::Failed
                | ReleaseStatus::Cancelled
                | ReleaseStatus::TimedOut
        )
    }

    pub fn is_running(&self) -> bool {
        matches!(self, ReleaseStatus::Running)
    }

    pub fn is_success(&self) -> bool {
        matches!(self, ReleaseStatus::Succeeded)
    }

    pub fn is_failure(&self) -> bool {
        matches!(self, ReleaseStatus::Failed)
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
            "QUEUED" => Ok(ReleaseStatus::Queued),
            "ASSIGNED" => Ok(ReleaseStatus::Assigned),
            "RUNNING" => Ok(ReleaseStatus::Running),
            "SUCCEEDED" => Ok(ReleaseStatus::Succeeded),
            "FAILED" => Ok(ReleaseStatus::Failed),
            "CANCELLED" => Ok(ReleaseStatus::Cancelled),
            "TIMED_OUT" => Ok(ReleaseStatus::TimedOut),
            // Backward compatibility with old status values
            "STAGED" => Ok(ReleaseStatus::Queued),
            "SUCCESS" => Ok(ReleaseStatus::Succeeded),
            "FAILURE" => Ok(ReleaseStatus::Failed),
            _ => Err(format!("unknown release status: {}", s)),
        }
    }
}

impl From<ReleaseStatus> for String {
    fn from(value: ReleaseStatus) -> Self {
        value.as_str().to_string()
    }
}
