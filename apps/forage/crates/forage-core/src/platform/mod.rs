use serde::{Deserialize, Serialize};

/// Validate that a slug (org name, project name) is safe for use in URLs and templates.
/// Allows lowercase alphanumeric, hyphens, max 64 chars. Must not be empty.
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
    pub dest_type: Option<DestinationType>,
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

/// Runtime status of a single pipeline stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineRunStageState {
    pub stage_id: String,
    pub depends_on: Vec<String>,
    pub stage_type: String, // "deploy", "wait", or "plan"
    pub status: String,     // "PENDING", "RUNNING", "SUCCEEDED", "FAILED", "CANCELLED", "AWAITING_APPROVAL"
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
}

/// Combined response from get_destination_states: destinations only.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeploymentStates {
    pub destinations: Vec<DestinationState>,
}

/// Full state of a release intent: pipeline stages + individual release steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseIntentState {
    pub release_intent_id: String,
    pub artifact_id: String,
    pub project: String,
    pub created_at: String,
    pub stages: Vec<PipelineRunStageState>,
    pub steps: Vec<ReleaseStepState>,
}

/// Status of an individual release step (deploy work item).
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

// ── Triggers (auto-release triggers) ────────────────────────────────

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

// ── Policies (deployment gating) ────────────────────────────────────

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
    Deploy { environment: String },
    Wait { duration_seconds: i64 },
    Plan { environment: String, auto_approve: bool },
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
    Other(String),
}

/// A user's notification preference for a specific event type + channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPreference {
    pub notification_type: String,
    pub channel: String,
    pub enabled: bool,
}

/// Trait for platform data from forest-server (organisations, projects, artifacts).
/// Separate from `ForestAuth` which handles identity.
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

    async fn create_destination(
        &self,
        access_token: &str,
        organisation: &str,
        name: &str,
        environment: &str,
        metadata: &std::collections::HashMap<String, String>,
        dest_type: Option<&DestinationType>,
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
    ) -> Result<(), PlatformError>;

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

    /// Get the spec (forest.cue) content for an artifact. Returns empty string if no spec was uploaded.
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

#[cfg(test)]
mod tests {
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
