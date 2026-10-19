pub mod users;

use std::{
    collections::{BTreeSet, HashMap},
    fmt::Display,
    ops::Deref,
};

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
            readme: String::new(),
            description: String::new(),
            metadata: Some(Default::default()),
        }
    }
}

pub struct Destination {
    pub organisation: String,
    pub name: String,
    pub environment: String,
    pub metadata: HashMap<String, String>,

    pub sensitive_keys: Vec<String>,

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
            sensitive_keys: Vec::new(),
            destination_type,
        }
    }

    pub fn with_sensitive_keys(mut self, sensitive_keys: Vec<String>) -> Self {
        self.sensitive_keys = sensitive_keys;
        self
    }

    /// Includes declared keys absent from `metadata`, so an unset credential still reads protected.
    pub fn sensitive_metadata_keys(&self) -> BTreeSet<&str> {
        self.destination_type
            .sensitive_field_names()
            .chain(self.sensitive_keys.iter().map(String::as_str))
            .collect()
    }

    pub fn is_sensitive_key(&self, key: &str) -> bool {
        self.destination_type
            .sensitive_field_names()
            .any(|f| f == key)
            || self.sensitive_keys.iter().any(|k| k == key)
    }

    /// Sensitivity is declared, never inferred from the key's name.
    pub fn partition_metadata(&self) -> (HashMap<String, String>, Vec<String>) {
        let mut visible = HashMap::with_capacity(self.metadata.len());
        let mut withheld = Vec::new();

        for (key, value) in &self.metadata {
            if self.is_sensitive_key(key) {
                withheld.push(key.clone());
            } else {
                visible.insert(key.clone(), value.clone());
            }
        }

        withheld.sort();
        (visible, withheld)
    }
}

impl Display for Destination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl From<Destination> for forest_grpc_interface::Destination {
    fn from(value: Destination) -> Self {
        let (metadata, sensitive_keys) = value.partition_metadata();

        Self {
            organisation: value.organisation,
            name: value.name,
            environment: value.environment,
            r#type: Some(value.destination_type.into()),
            metadata,
            sensitive_keys,
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
    pub sensitive: bool,
}

pub struct DestinationType {
    pub organisation: String,
    pub name: String,
    pub version: usize,
    pub description: String,
    pub fields: Vec<MetadataFieldSchema>,
}

impl DestinationType {
    /// The one spelling of a destination type; CLI, server and `forest.cue` must all agree on it.
    pub fn qualified(&self) -> String {
        format!("{}/{}@{}", self.organisation, self.name, self.version)
    }

    pub fn sensitive_field_names(&self) -> impl Iterator<Item = &str> {
        self.fields
            .iter()
            .filter(|f| f.sensitive)
            .map(|f| f.name.as_str())
    }
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
            sensitive: value.sensitive,
        }
    }
}

impl From<forest_grpc_interface::MetadataFieldSchema> for MetadataFieldSchema {
    fn from(value: forest_grpc_interface::MetadataFieldSchema) -> Self {
        Self {
            name: value.name,
            label: value.label,
            description: value.description,
            required: value.required,
            field_type: value.field_type,
            default_value: value.default_value,
            sensitive: value.sensitive,
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
            fields: value.fields.into_iter().map(Into::into).collect(),
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
    Superseded,
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
            ReleaseStatus::Superseded => "SUPERSEDED",
        }
    }

    pub fn is_finalized(&self) -> bool {
        matches!(
            self,
            ReleaseStatus::Succeeded
                | ReleaseStatus::Failed
                | ReleaseStatus::Cancelled
                | ReleaseStatus::TimedOut
                | ReleaseStatus::Superseded
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
            "SUPERSEDED" => Ok(ReleaseStatus::Superseded),
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

#[cfg(test)]
mod sensitive_metadata_tests {
    use super::*;

    fn field(name: &str, sensitive: bool) -> MetadataFieldSchema {
        MetadataFieldSchema {
            name: name.into(),
            label: name.into(),
            description: String::new(),
            required: false,
            field_type: "text".into(),
            default_value: String::new(),
            sensitive,
        }
    }

    fn destination_type(fields: Vec<MetadataFieldSchema>) -> DestinationType {
        DestinationType {
            organisation: "forest".into(),
            name: "flux".into(),
            version: 1,
            description: String::new(),
            fields,
        }
    }

    fn destination(metadata: &[(&str, &str)], fields: Vec<MetadataFieldSchema>) -> Destination {
        Destination::new(
            "understory",
            "flux-dev",
            "dev",
            metadata
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            destination_type(fields),
        )
    }

    #[test]
    fn type_declared_sensitive_field_is_withheld() {
        let dest = destination(
            &[("cluster_name", "prod-eu"), ("git_token", "ghp_secret")],
            vec![field("cluster_name", false), field("git_token", true)],
        );

        let (visible, withheld) = dest.partition_metadata();

        assert_eq!(visible.get("cluster_name").map(String::as_str), Some("prod-eu"));
        assert!(!visible.contains_key("git_token"));
        assert_eq!(withheld, vec!["git_token".to_string()]);
    }

    #[test]
    fn keys_outside_the_schema_stay_visible_unless_declared() {
        let dest = destination(
            &[("infra_environment", "dev"), ("some_token", "shhh")],
            vec![field("tf_workspace", false)],
        );

        let (visible, withheld) = dest.partition_metadata();

        assert!(withheld.is_empty());
        assert_eq!(visible.get("some_token").map(String::as_str), Some("shhh"));
    }

    #[test]
    fn destination_can_declare_free_form_keys_sensitive() {
        let dest = destination(
            &[
                ("tf_workspace", "platform-dev"),
                ("infra_environment", "dev"),
                ("aws_access_key_id", "AKIA..."),
                ("aws_secret_access_key", "wJal..."),
                ("cloudflare_token", "cf_..."),
            ],
            vec![field("tf_workspace", false)],
        )
        .with_sensitive_keys(vec![
            "aws_access_key_id".into(),
            "aws_secret_access_key".into(),
            "cloudflare_token".into(),
        ]);

        let (visible, withheld) = dest.partition_metadata();

        assert_eq!(
            withheld,
            vec![
                "aws_access_key_id".to_string(),
                "aws_secret_access_key".to_string(),
                "cloudflare_token".to_string(),
            ]
        );
        assert_eq!(visible.len(), 2);
        assert_eq!(
            visible.get("tf_workspace").map(String::as_str),
            Some("platform-dev")
        );
        assert_eq!(
            visible.get("infra_environment").map(String::as_str),
            Some("dev")
        );
    }

    #[test]
    fn type_and_destination_declarations_are_unioned_without_duplicates() {
        let dest = destination(
            &[("git_token", "ghp"), ("cf_token", "cf")],
            vec![field("git_token", true)],
        )
        .with_sensitive_keys(vec!["git_token".into(), "cf_token".into()]);

        assert_eq!(
            dest.sensitive_metadata_keys().into_iter().collect::<Vec<_>>(),
            vec!["cf_token", "git_token"]
        );
        assert!(dest.partition_metadata().0.is_empty());
    }

    #[test]
    fn grpc_conversion_never_serialises_a_sensitive_value() {
        let dest = destination(
            &[("namespace", "fungus"), ("webhook_secret", "hmac-secret")],
            vec![field("namespace", false), field("webhook_secret", true)],
        );

        let wire: forest_grpc_interface::Destination = dest.into();

        assert_eq!(wire.sensitive_keys, vec!["webhook_secret".to_string()]);
        assert!(!wire.metadata.contains_key("webhook_secret"));
        assert!(
            !format!("{wire:?}").contains("hmac-secret"),
            "sensitive value must not survive into the wire message"
        );
    }

    #[test]
    fn redaction_is_presentation_only_the_model_keeps_every_value() {
        let dest = destination(
            &[("namespace", "fungus"), ("git_token", "ghp_secret")],
            vec![field("namespace", false), field("git_token", true)],
        );

        assert_eq!(
            dest.metadata.get("git_token").map(String::as_str),
            Some("ghp_secret"),
            "the model must still carry the value for the deploy path"
        );
        assert!(dest.is_sensitive_key("git_token"));
    }

    #[test]
    fn destination_type_round_trips_the_sensitive_flag_over_grpc() {
        let original = destination_type(vec![field("git_token", true), field("namespace", false)]);

        let wire: forest_grpc_interface::DestinationType = original.into();
        assert!(wire.fields.iter().any(|f| f.name == "git_token" && f.sensitive));

        let back: DestinationType = wire.into();
        assert_eq!(
            back.sensitive_field_names().collect::<Vec<_>>(),
            vec!["git_token"]
        );
        assert_eq!(back.fields.len(), 2);
    }
}
