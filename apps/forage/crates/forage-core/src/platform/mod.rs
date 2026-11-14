use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub fn validate_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Organisation {
    pub organisation_id: String,
    pub name: String,
    pub role: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Project {
    pub organisation: String,
    pub project: String,
    #[serde(default)]
    pub readme: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub metadata: ProjectMetadata,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMetadata {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub git_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub homepage: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub docs_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub support_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub domain: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl ProjectMetadata {
    pub fn is_empty(&self) -> bool {
        self.git_url.is_empty()
            && self.homepage.is_empty()
            && self.docs_url.is_empty()
            && self.support_url.is_empty()
            && self.domain.is_empty()
            && self.owner.is_empty()
            && self.tags.is_empty()
    }
}

pub fn prettify_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let without_www = without_scheme
        .strip_prefix("www.")
        .unwrap_or(without_scheme);
    let trailing_slash_trimmed = without_www.trim_end_matches('/');

    for forge in ["github.com/", "gitlab.com/"] {
        if let Some(rest) = trailing_slash_trimmed.strip_prefix(forge) {
            let mut parts = rest.split('/');
            let org = parts.next().unwrap_or("");
            let repo = parts.next().unwrap_or("");
            if !org.is_empty() && !repo.is_empty() {
                return format!("{org}/{repo}");
            }
            break;
        }
    }

    cap_chars(trailing_slash_trimmed, 48)
}

fn cap_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod prettify_url_tests {
    use super::*;

    #[test]
    fn empty_in_empty_out() {
        assert_eq!(prettify_url(""), "");
        assert_eq!(prettify_url("   "), "");
    }

    #[test]
    fn github_repo_collapses_to_org_slash_repo() {
        assert_eq!(
            prettify_url("https://github.com/rawpotion/forest-hello"),
            "rawpotion/forest-hello"
        );
        assert_eq!(
            prettify_url("https://github.com/rawpotion/forest-hello/issues"),
            "rawpotion/forest-hello"
        );
    }

    #[test]
    fn gitlab_repo_collapses_too() {
        assert_eq!(
            prettify_url("https://gitlab.com/group/project"),
            "group/project"
        );
    }

    #[test]
    fn homepage_strips_scheme_and_www() {
        assert_eq!(prettify_url("https://www.example.com/"), "example.com");
        assert_eq!(
            prettify_url("http://forest.rawpotion.io"),
            "forest.rawpotion.io"
        );
    }

    #[test]
    fn long_url_is_capped_with_ellipsis() {
        let pretty =
            prettify_url("https://very-long.example.com/path/that/keeps/going/and/going/forever");
        assert!(pretty.chars().count() <= 48);
        assert!(pretty.ends_with('…'));
    }

    #[test]
    fn malformed_url_falls_back_to_raw() {
        assert_eq!(prettify_url("not a url"), "not a url");
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub artifact_id: String,
    pub slug: String,
    pub context: ArtifactContext,
    #[serde(default)]
    pub source: Option<ArtifactSource>,
    #[serde(default)]
    pub git_ref: Option<ArtifactRef>,
    #[serde(default)]
    pub destinations: Vec<ArtifactDestination>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactContext {
    pub title: String,
    pub description: Option<String>,
    #[serde(default)]
    pub web: Option<String>,
    #[serde(default)]
    pub pr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSource {
    pub user: Option<String>,
    pub email: Option<String>,
    pub source_type: Option<String>,
    pub run_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub commit_sha: String,
    pub branch: Option<String>,
    pub commit_message: Option<String>,
    pub version: Option<String>,
    pub repo_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactDestination {
    pub name: String,
    pub environment: String,
    #[serde(default)]
    pub type_organisation: Option<String>,
    #[serde(default)]
    pub type_name: Option<String>,
    #[serde(default)]
    pub type_version: Option<u64>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgMember {
    pub user_id: String,
    pub username: String,
    pub role: String,
    pub joined_at: Option<String>,
}

/// `can_change` is Forest's own admin decision; render the form only when it is true.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Visibility {
    pub organisation: String,
    pub project: Option<String>,
    pub can_change: bool,
}

impl Visibility {
    pub fn organisation_is_public(&self) -> bool {
        self.organisation == "public"
    }

    pub fn project_is_public(&self) -> bool {
        self.project.as_deref() == Some("public")
    }

    pub fn component_is_public(&self) -> bool {
        self.organisation_is_public() || self.project_is_public()
    }
}

pub fn validate_visibility(value: &str) -> bool {
    matches!(value, "public" | "private")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowedDomain {
    pub domain: String,
    pub policy: String,
    pub dns_verified: bool,
    pub dns_verification_token: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerifyDomainOutcome {
    Verified,
    AlreadyVerified,
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinOffer {
    pub organisation_id: String,
    pub organisation_name: String,
    pub matched_domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub organisation: String,
    pub name: String,
    pub description: Option<String>,
    pub sort_order: i32,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Destination {
    pub name: String,
    pub environment: String,
    pub organisation: String,
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub sensitive_keys: Vec<String>,
    #[serde(default)]
    pub dest_type: Option<DestinationType>,
}

pub struct NewDestination<'a> {
    pub name: &'a str,
    pub environment: &'a str,
    pub metadata: &'a std::collections::HashMap<String, String>,
    pub sensitive_keys: &'a [String],
    pub dest_type: Option<&'a DestinationType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestinationType {
    pub organisation: String,
    pub name: String,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestinationTypeInfo {
    pub organisation: String,
    pub name: String,
    pub version: u64,
    pub description: String,
    pub fields: Vec<MetadataFieldDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataFieldDef {
    pub name: String,
    pub label: String,
    pub description: String,
    pub required: bool,
    pub field_type: String,
    pub default_value: String,
    #[serde(default)]
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestinationState {
    pub destination_id: String,
    pub destination_name: String,
    pub environment: String,
    pub release_id: Option<String>,
    pub artifact_id: Option<String>,
    pub status: Option<String>,
    pub error_message: Option<String>,
    pub queued_at: Option<String>,
    pub completed_at: Option<String>,
    pub queue_position: Option<i32>,
    #[serde(default)]
    pub started_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PipelineRunStageState {
    pub stage_id: String,
    pub depends_on: Vec<String>,
    pub stage_type: String,
    pub status: String,
    pub environment: Option<String>,
    pub duration_seconds: Option<i64>,
    pub queued_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub error_message: Option<String>,
    pub wait_until: Option<String>,
    #[serde(default)]
    pub release_ids: Vec<String>,
    #[serde(default)]
    pub approval_status: Option<String>,
    #[serde(default)]
    pub auto_approve: Option<bool>,
    #[serde(default)]
    pub gate_deadline: Option<String>,
    #[serde(default)]
    pub gate_waiting_on: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeploymentStates {
    pub destinations: Vec<DestinationState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseIntentState {
    pub release_intent_id: String,
    pub artifact_id: String,
    pub project: String,
    pub created_at: String,
    pub stages: Vec<PipelineRunStageState>,
    pub steps: Vec<ReleaseStepState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseStepState {
    pub release_id: String,
    pub stage_id: Option<String>,
    pub destination_name: String,
    pub environment: String,
    pub status: String,
    pub queued_at: Option<String>,
    pub assigned_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectSelector {
    pub include_projects: Vec<String>,
    pub exclude_projects: Vec<String>,
    pub name_regex: Option<String>,
    pub metadata_match: BTreeMap<String, String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgRuleSet {
    pub organisation: String,
    pub name: String,
    pub enabled: bool,
    pub selector: ProjectSelector,
    pub policies: Vec<OrgPolicyRule>,
    pub triggers: Vec<OrgTriggerRule>,
    pub release_pipelines: Vec<OrgReleasePipelineRule>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgPolicyRule {
    pub name: String,
    pub enabled: bool,
    pub config: PolicyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgTriggerRule {
    pub name: String,
    pub enabled: bool,
    pub branch_pattern: Option<String>,
    pub title_pattern: Option<String>,
    pub author_pattern: Option<String>,
    pub commit_message_pattern: Option<String>,
    pub source_type_pattern: Option<String>,
    pub target_environments: Vec<String>,
    pub target_destinations: Vec<String>,
    pub force_release: bool,
    pub use_pipeline: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgReleasePipelineRule {
    pub name: String,
    pub enabled: bool,
    pub stages: Vec<PipelineStage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateOrgRuleSetInput {
    pub name: String,
    pub enabled: bool,
    pub selector: ProjectSelector,
    pub policies: Vec<OrgPolicyRule>,
    pub triggers: Vec<OrgTriggerRule>,
    pub release_pipelines: Vec<OrgReleasePipelineRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateOrgRuleSetInput {
    pub name: String,
    pub enabled: bool,
    pub selector: ProjectSelector,
    pub policies: Vec<OrgPolicyRule>,
    pub triggers: Vec<OrgTriggerRule>,
    pub release_pipelines: Vec<OrgReleasePipelineRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub branch_pattern: Option<String>,
    pub title_pattern: Option<String>,
    pub author_pattern: Option<String>,
    pub commit_message_pattern: Option<String>,
    pub source_type_pattern: Option<String>,
    pub target_environments: Vec<String>,
    pub target_destinations: Vec<String>,
    pub force_release: bool,
    pub use_pipeline: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTriggerInput {
    pub name: String,
    pub branch_pattern: Option<String>,
    pub title_pattern: Option<String>,
    pub author_pattern: Option<String>,
    pub commit_message_pattern: Option<String>,
    pub source_type_pattern: Option<String>,
    pub target_environments: Vec<String>,
    pub target_destinations: Vec<String>,
    pub force_release: bool,
    pub use_pipeline: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTriggerInput {
    pub enabled: Option<bool>,
    pub branch_pattern: Option<String>,
    pub title_pattern: Option<String>,
    pub author_pattern: Option<String>,
    pub commit_message_pattern: Option<String>,
    pub source_type_pattern: Option<String>,
    pub target_environments: Vec<String>,
    pub target_destinations: Vec<String>,
    pub force_release: Option<bool>,
    pub use_pipeline: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub policy_type: String,
    pub config: PolicyConfig,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PolicyConfig {
    SoakTime {
        source_environment: String,
        target_environment: String,
        duration_seconds: i64,
    },
    BranchRestriction {
        target_environment: String,
        branch_pattern: String,
    },
    Approval {
        target_environment: String,
        required_approvals: i32,
    },
    SupersedePending {
        target_environment: String,
        same_branch_only: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePolicyInput {
    pub name: String,
    pub config: PolicyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePolicyInput {
    pub enabled: Option<bool>,
    pub config: Option<PolicyConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyEvaluation {
    pub policy_name: String,
    pub policy_type: String,
    pub passed: bool,
    pub reason: String,
    #[serde(default)]
    pub approval_state: Option<ApprovalState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalState {
    pub required_approvals: i32,
    pub current_approvals: i32,
    pub decisions: Vec<ApprovalDecisionEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalDecisionEntry {
    pub user_id: String,
    pub username: String,
    pub decision: String,
    pub decided_at: String,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStage {
    pub id: String,
    pub depends_on: Vec<String>,
    pub config: PipelineStageConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PipelineStageConfig {
    Deploy {
        environment: String,
    },
    Wait {
        duration_seconds: i64,
    },
    Plan {
        environment: String,
        auto_approve: bool,
        #[serde(default)]
        destroy: bool,
    },
    Gate {
        requires: Vec<SignalRequirement>,
        timeout_seconds: i64,
        on_timeout: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalRequirement {
    pub signal: String,
    #[serde(default)]
    pub accept: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleasePipeline {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub stages: Vec<PipelineStage>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateReleasePipelineInput {
    pub name: String,
    pub stages: Vec<PipelineStage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateReleasePipelineInput {
    pub enabled: Option<bool>,
    pub stages: Option<Vec<PipelineStage>>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum PlatformError {
    #[error("not authenticated")]
    NotAuthenticated,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("service unavailable: {0}")]
    Unavailable(String),

    #[error("{0}")]
    InvalidArgument(String),

    #[error("{0}")]
    AlreadyExists(String),

    #[error("{0}")]
    PermissionDenied(String),

    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPreference {
    pub notification_type: String,
    pub channel: String,
    pub enabled: bool,
}

#[async_trait::async_trait]
pub trait ForestPlatform: Send + Sync {
    async fn list_my_organisations(
        &self,
        access_token: &str,
    ) -> Result<Vec<Organisation>, PlatformError>;

    async fn list_projects(
        &self,
        access_token: &str,
        organisation: &str,
    ) -> Result<Vec<String>, PlatformError>;

    async fn get_project(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
    ) -> Result<Option<Project>, PlatformError>;

    async fn list_artifacts(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
    ) -> Result<Vec<Artifact>, PlatformError>;

    async fn create_organisation(
        &self,
        access_token: &str,
        name: &str,
    ) -> Result<String, PlatformError>;

    async fn list_members(
        &self,
        access_token: &str,
        organisation_id: &str,
    ) -> Result<Vec<OrgMember>, PlatformError>;

    async fn add_member(
        &self,
        access_token: &str,
        organisation_id: &str,
        user_id: &str,
        role: &str,
    ) -> Result<OrgMember, PlatformError>;

    async fn remove_member(
        &self,
        access_token: &str,
        organisation_id: &str,
        user_id: &str,
    ) -> Result<(), PlatformError>;

    async fn update_member_role(
        &self,
        access_token: &str,
        organisation_id: &str,
        user_id: &str,
        role: &str,
    ) -> Result<OrgMember, PlatformError>;

    async fn get_artifact_by_slug(
        &self,
        access_token: &str,
        slug: &str,
    ) -> Result<Artifact, PlatformError>;

    async fn list_environments(
        &self,
        access_token: &str,
        organisation: &str,
    ) -> Result<Vec<Environment>, PlatformError>;

    async fn list_destinations(
        &self,
        access_token: &str,
        organisation: &str,
    ) -> Result<Vec<Destination>, PlatformError>;

    async fn create_environment(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
        description: Option<&str>,
        sort_order: i32,
    ) -> Result<Environment, PlatformError>;

    async fn update_environment(
        &self,
        access_token: &str,
        id: &str,
        description: Option<&str>,
        sort_order: Option<i32>,
    ) -> Result<Environment, PlatformError>;

    async fn create_destination(
        &self,
        access_token: &str,
        organisation: &str,
        dest: NewDestination<'_>,
    ) -> Result<(), PlatformError>;

    async fn list_destination_types(
        &self,
        access_token: &str,
    ) -> Result<Vec<DestinationTypeInfo>, PlatformError>;

    async fn update_destination(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
        metadata: &std::collections::HashMap<String, String>,
        sensitive_keys: Option<&[String]>,
    ) -> Result<(), PlatformError>;

    async fn reveal_destination_metadata(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
        key: &str,
    ) -> Result<String, PlatformError>;

    async fn get_destination_states(
        &self,
        access_token: &str,
        organisation: &str,
        project: Option<&str>,
    ) -> Result<DeploymentStates, PlatformError>;

    async fn get_release_intent_states(
        &self,
        access_token: &str,
        organisation: &str,
        project: Option<&str>,
        include_completed: bool,
    ) -> Result<Vec<ReleaseIntentState>, PlatformError>;

    async fn release_artifact(
        &self,
        access_token: &str,
        artifact_id: &str,
        destinations: &[String],
        environments: &[String],
        use_pipeline: bool,
    ) -> Result<(), PlatformError>;

    async fn list_org_rule_sets(
        &self,
        access_token: &str,
        organisation: &str,
    ) -> Result<Vec<OrgRuleSet>, PlatformError>;

    async fn create_org_rule_set(
        &self,
        access_token: &str,
        organisation: &str,
        input: &CreateOrgRuleSetInput,
    ) -> Result<OrgRuleSet, PlatformError>;

    async fn update_org_rule_set(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
        input: &UpdateOrgRuleSetInput,
    ) -> Result<OrgRuleSet, PlatformError>;

    async fn delete_org_rule_set(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
    ) -> Result<(), PlatformError>;

    async fn list_triggers(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
    ) -> Result<Vec<Trigger>, PlatformError>;

    async fn create_trigger(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        input: &CreateTriggerInput,
    ) -> Result<Trigger, PlatformError>;

    async fn update_trigger(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        name: &str,
        input: &UpdateTriggerInput,
    ) -> Result<Trigger, PlatformError>;

    async fn delete_trigger(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        name: &str,
    ) -> Result<(), PlatformError>;

    async fn list_policies(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
    ) -> Result<Vec<Policy>, PlatformError>;

    async fn create_policy(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        input: &CreatePolicyInput,
    ) -> Result<Policy, PlatformError>;

    async fn update_policy(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        name: &str,
        input: &UpdatePolicyInput,
    ) -> Result<Policy, PlatformError>;

    async fn delete_policy(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        name: &str,
    ) -> Result<(), PlatformError>;

    async fn list_release_pipelines(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
    ) -> Result<Vec<ReleasePipeline>, PlatformError>;

    async fn create_release_pipeline(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        input: &CreateReleasePipelineInput,
    ) -> Result<ReleasePipeline, PlatformError>;

    async fn update_release_pipeline(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        name: &str,
        input: &UpdateReleasePipelineInput,
    ) -> Result<ReleasePipeline, PlatformError>;

    async fn delete_release_pipeline(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        name: &str,
    ) -> Result<(), PlatformError>;

    /// Empty string, not an error, when no spec was uploaded.
    async fn get_artifact_spec(
        &self,
        access_token: &str,
        artifact_id: &str,
    ) -> Result<String, PlatformError>;

    async fn get_notification_preferences(
        &self,
        access_token: &str,
    ) -> Result<Vec<NotificationPreference>, PlatformError>;

    async fn set_notification_preference(
        &self,
        access_token: &str,
        notification_type: &str,
        channel: &str,
        enabled: bool,
    ) -> Result<(), PlatformError>;

    async fn reset_notification_preference(
        &self,
        access_token: &str,
        notification_type: &str,
        channel: &str,
    ) -> Result<(), PlatformError>;

    async fn evaluate_policies(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        target_environment: &str,
        release_intent_id: Option<&str>,
    ) -> Result<Vec<PolicyEvaluation>, PlatformError>;

    async fn approve_release(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        release_intent_id: &str,
        target_environment: &str,
        comment: Option<&str>,
        force_bypass: bool,
    ) -> Result<ApprovalState, PlatformError>;

    async fn reject_release(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        release_intent_id: &str,
        target_environment: &str,
        comment: Option<&str>,
    ) -> Result<ApprovalState, PlatformError>;

    async fn get_approval_state(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        release_intent_id: &str,
        target_environment: &str,
    ) -> Result<ApprovalState, PlatformError>;

    async fn approve_plan_stage(
        &self,
        access_token: &str,
        release_intent_id: &str,
        stage_id: &str,
    ) -> Result<(), PlatformError>;

    async fn reject_plan_stage(
        &self,
        access_token: &str,
        release_intent_id: &str,
        stage_id: &str,
        reason: Option<&str>,
    ) -> Result<(), PlatformError>;

    async fn get_plan_output(
        &self,
        access_token: &str,
        release_intent_id: &str,
        stage_id: &str,
    ) -> Result<PlanOutput, PlatformError>;

    async fn get_visibility(
        &self,
        access_token: &str,
        organisation: &str,
        project: Option<&str>,
    ) -> Result<Visibility, PlatformError>;

    async fn set_organisation_visibility(
        &self,
        access_token: &str,
        organisation: &str,
        visibility: &str,
    ) -> Result<(), PlatformError>;

    async fn set_project_visibility(
        &self,
        access_token: &str,
        organisation: &str,
        project: &str,
        visibility: &str,
    ) -> Result<(), PlatformError>;

    async fn list_allowed_domains(
        &self,
        access_token: &str,
        organisation_id: &str,
    ) -> Result<Vec<AllowedDomain>, PlatformError>;

    async fn add_allowed_domain(
        &self,
        access_token: &str,
        organisation_id: &str,
        domain: &str,
    ) -> Result<AllowedDomain, PlatformError>;

    async fn remove_allowed_domain(
        &self,
        access_token: &str,
        organisation_id: &str,
        domain: &str,
    ) -> Result<bool, PlatformError>;

    async fn verify_allowed_domain(
        &self,
        access_token: &str,
        organisation_id: &str,
        domain: &str,
    ) -> Result<VerifyDomainOutcome, PlatformError>;

    async fn list_join_offers(&self, access_token: &str) -> Result<Vec<JoinOffer>, PlatformError>;

    async fn accept_join_offer(
        &self,
        access_token: &str,
        organisation_id: &str,
    ) -> Result<OrgMember, PlatformError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanOutput {
    pub plan_output: String,
    pub status: String,
    pub outputs: Vec<PlanDestinationOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanDestinationOutput {
    pub destination_id: String,
    pub destination_name: String,
    pub plan_output: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthApp {
    pub app_id: String,
    pub organisation_id: String,
    pub name: String,
    pub description: String,
    pub homepage_url: String,
    pub client_id: String,
    pub redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
    pub grant_types: Vec<String>,
    #[serde(default)]
    pub public: bool,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct CreatedOAuthApp {
    pub app: OAuthApp,
    pub client_secret: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthClientInfo {
    pub app_id: String,
    pub organisation_id: String,
    pub name: String,
    pub description: String,
    pub homepage_url: String,
    pub redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
    pub public: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthIssuedTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in_seconds: i64,
    pub scopes: Vec<String>,
    pub id_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthDeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in_seconds: i64,
    pub interval_seconds: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOAuthDeviceAuthorization {
    pub client_id: String,
    pub name: String,
    pub homepage_url: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthDeviceToken {
    Issued(OAuthIssuedTokens),
    AuthorizationPending,
    SlowDown,
    AccessDenied,
    ExpiredToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPrincipal {
    pub app_id: String,
    pub organisation_id: String,
    pub scopes: Vec<String>,
    pub user: Option<DelegatingUser>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatingUser {
    pub user_id: String,
    pub username: String,
    pub organisations: Vec<DelegatingUserOrganisation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatingUserOrganisation {
    pub organisation_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryLookup {
    Email(String),
    Provider {
        provider: String,
        provider_user_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryUser {
    pub user_id: String,
    pub username: String,
    pub emails: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthClientToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in_seconds: i64,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthGrant {
    pub app_id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub authorized_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthUserinfo {
    pub sub: String,
    pub username: Option<String>,
    pub profile_picture_url: Option<String>,
    pub email: Option<String>,
    pub emails: Vec<String>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OAuthFlowError {
    #[error("invalid_client")]
    InvalidClient,
    #[error("invalid_grant")]
    InvalidGrant,
    #[error("invalid_scope")]
    InvalidScope,
    #[error("invalid_request: {0}")]
    InvalidRequest(String),
    #[error("server_error: {0}")]
    ServerError(String),
}

#[async_trait::async_trait]
pub trait ForestOAuthApps: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    async fn create_oauth_app(
        &self,
        access_token: &str,
        organisation_id: &str,
        name: &str,
        description: &str,
        homepage_url: &str,
        redirect_uris: &[String],
        scopes: &[String],
        grant_types: &[String],
    ) -> Result<CreatedOAuthApp, PlatformError>;

    async fn list_oauth_apps(
        &self,
        access_token: &str,
        organisation_id: &str,
    ) -> Result<Vec<OAuthApp>, PlatformError>;

    async fn get_oauth_app(
        &self,
        access_token: &str,
        organisation_id: &str,
        app_id: &str,
    ) -> Result<OAuthApp, PlatformError>;

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    async fn update_oauth_app(
        &self,
        access_token: &str,
        organisation_id: &str,
        app_id: &str,
        name: &str,
        description: &str,
        homepage_url: &str,
        redirect_uris: &[String],
        scopes: &[String],
        grant_types: &[String],
    ) -> Result<OAuthApp, PlatformError>;

    async fn rotate_oauth_app_secret(
        &self,
        access_token: &str,
        organisation_id: &str,
        app_id: &str,
    ) -> Result<CreatedOAuthApp, PlatformError>;

    async fn delete_oauth_app(
        &self,
        access_token: &str,
        organisation_id: &str,
        app_id: &str,
    ) -> Result<(), PlatformError>;

    async fn issue_client_credentials_token(
        &self,
        client_id: &str,
        client_secret: &str,
        scopes: &[String],
    ) -> Result<OAuthClientToken, OAuthFlowError>;

    /// `None` for unknown, expired and revoked tokens alike, deliberately indistinguishable.
    async fn introspect_client_token(
        &self,
        access_token: &str,
    ) -> Result<Option<ClientPrincipal>, OAuthFlowError>;

    async fn resolve_directory_user(
        &self,
        lookup: DirectoryLookup,
    ) -> Result<Option<DirectoryUser>, PlatformError>;

    async fn lookup_oauth_client(
        &self,
        client_id: &str,
    ) -> Result<Option<OAuthClientInfo>, PlatformError>;

    #[allow(clippy::too_many_arguments)]
    async fn create_oauth_authorization_code(
        &self,
        client_id: &str,
        user_id: &str,
        redirect_uri: &str,
        scopes: &[String],
        code_challenge: Option<&str>,
        code_challenge_method: Option<&str>,
        nonce: Option<&str>,
    ) -> Result<String, OAuthFlowError>;

    async fn exchange_oauth_code(
        &self,
        client_id: &str,
        client_secret: &str,
        code: &str,
        redirect_uri: &str,
        code_verifier: Option<&str>,
    ) -> Result<OAuthIssuedTokens, OAuthFlowError>;

    async fn oauth_userinfo(&self, access_token: &str) -> Result<OAuthUserinfo, OAuthFlowError>;

    async fn refresh_oauth_token(
        &self,
        client_id: &str,
        client_secret: &str,
        refresh_token: &str,
    ) -> Result<OAuthIssuedTokens, OAuthFlowError>;

    async fn revoke_oauth_grant(&self, user_id: &str, app_id: &str) -> Result<u32, PlatformError>;

    async fn list_oauth_grants(&self, user_id: &str) -> Result<Vec<OAuthGrant>, PlatformError>;

    async fn get_oauth_consent(
        &self,
        client_id: &str,
        user_id: &str,
    ) -> Result<Vec<String>, PlatformError>;

    async fn initiate_oauth_device_authorization(
        &self,
        client_id: &str,
        client_secret: &str,
        scopes: &[String],
    ) -> Result<OAuthDeviceAuthorization, OAuthFlowError>;

    /// `None` for anything but an app's pending grant, a Forest CLI login included.
    async fn lookup_oauth_device_authorization(
        &self,
        user_code: &str,
    ) -> Result<Option<PendingOAuthDeviceAuthorization>, PlatformError>;

    async fn approve_oauth_device_authorization(
        &self,
        user_code: &str,
        user_id: &str,
        approving_ip: &str,
        approving_user_agent: &str,
    ) -> Result<(), PlatformError>;

    async fn deny_oauth_device_authorization(
        &self,
        user_code: &str,
        user_id: &str,
    ) -> Result<(), PlatformError>;

    async fn exchange_oauth_device_code(
        &self,
        client_id: &str,
        client_secret: &str,
        device_code: &str,
    ) -> Result<OAuthDeviceToken, OAuthFlowError>;
}

#[cfg(test)]
mod tests {
    use super::{Visibility, validate_visibility};

    fn vis(organisation: &str, project: Option<&str>) -> Visibility {
        Visibility {
            organisation: organisation.into(),
            project: project.map(Into::into),
            can_change: false,
        }
    }

    #[test]
    fn a_component_is_public_through_its_own_setting_or_its_organisation() {
        assert!(!vis("private", Some("private")).component_is_public());
        assert!(vis("private", Some("public")).component_is_public());
        assert!(vis("public", Some("private")).component_is_public());
        assert!(vis("public", None).component_is_public());
    }

    #[test]
    fn only_public_and_private_are_visibilities() {
        assert!(validate_visibility("public"));
        assert!(validate_visibility("private"));
        assert!(!validate_visibility("internal"));
        assert!(!validate_visibility("PUBLIC"));
        assert!(!validate_visibility(""));
    }

    use super::*;

    #[test]
    fn valid_slugs() {
        assert!(validate_slug("my-org"));
        assert!(validate_slug("a"));
        assert!(validate_slug("abc123"));
        assert!(validate_slug("my-cool-project-2"));
    }

    #[test]
    fn invalid_slugs() {
        assert!(!validate_slug(""));
        assert!(!validate_slug("-starts-with-dash"));
        assert!(!validate_slug("ends-with-dash-"));
        assert!(!validate_slug("UPPERCASE"));
        assert!(!validate_slug("has spaces"));
        assert!(!validate_slug("has_underscores"));
        assert!(!validate_slug("has.dots"));
        assert!(!validate_slug(&"a".repeat(65)));
    }

    #[test]
    fn max_length_slug_is_valid() {
        assert!(validate_slug(&"a".repeat(64)));
    }
}
