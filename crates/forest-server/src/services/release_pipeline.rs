use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::State;

// ── DAG definition types (stored in release_pipelines.stages) ────────────

/// The full pipeline definition: a map of stage-id -> stage definition.
pub type PipelineStages = HashMap<String, StageDefinition>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageDefinition {
    /// Which stages must complete before this one starts.
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// The stage configuration — determines both the type and its parameters.
    #[serde(flatten)]
    pub config: StageConfig,
}

/// Tagged enum for stage types. Each variant carries exactly the config it needs.
/// Serializes with `"type": "deploy"` / `"type": "wait"` discriminator, and
/// the variant fields are flattened into the parent object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StageConfig {
    Deploy {
        environment: String,
    },
    Wait {
        duration_seconds: i64,
    },
    Plan {
        environment: String,
        /// When true, auto-approve after plan succeeds (no manual gate).
        #[serde(default)]
        auto_approve: bool,
    },
}

impl StageDefinition {
    pub fn deploy(environment: impl Into<String>, depends_on: Vec<String>) -> Self {
        Self {
            depends_on,
            config: StageConfig::Deploy {
                environment: environment.into(),
            },
        }
    }

    pub fn wait(duration_seconds: i64, depends_on: Vec<String>) -> Self {
        Self {
            depends_on,
            config: StageConfig::Wait { duration_seconds },
        }
    }

    pub fn plan(environment: impl Into<String>, auto_approve: bool, depends_on: Vec<String>) -> Self {
        Self {
            depends_on,
            config: StageConfig::Plan {
                environment: environment.into(),
                auto_approve,
            },
        }
    }
}

// ── Runtime state types (stored in release_intents.stage_states) ─────────

pub type StageStates = HashMap<String, StageState>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageState {
    pub status: StageStatus,

    /// When this stage became eligible to run (dependencies met).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_at: Option<String>,

    /// When this stage actually started executing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,

    /// When this stage reached a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,

    /// UUIDs of release_states rows created for this stage (deploy stages).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_ids: Option<Vec<String>>,

    /// For wait stages: ISO8601 timestamp when the wait expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_until: Option<String>,

    /// For plan stages: tracks approval lifecycle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_status: Option<ApprovalStatus>,

    /// When approval was granted/rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_at: Option<String>,

    /// Who approved/rejected (actor_id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApprovalStatus {
    AwaitingApproval,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StageStatus {
    Pending,
    Active,
    Succeeded,
    Failed,
    Cancelled,
}

impl StageState {
    pub fn pending() -> Self {
        Self {
            status: StageStatus::Pending,
            queued_at: None,
            started_at: None,
            completed_at: None,
            error_message: None,
            release_ids: None,
            wait_until: None,
            approval_status: None,
            approval_at: None,
            approved_by: None,
        }
    }
}

/// Simple stage type discriminator (derived from StageConfig).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageType {
    Deploy,
    Wait,
    Plan,
}

impl StageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Deploy => "deploy",
            Self::Wait => "wait",
            Self::Plan => "plan",
        }
    }
}

impl std::fmt::Display for StageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl StageConfig {
    pub fn stage_type(&self) -> StageType {
        match self {
            Self::Deploy { .. } => StageType::Deploy,
            Self::Wait { .. } => StageType::Wait,
            Self::Plan { .. } => StageType::Plan,
        }
    }
}

// ── DAG validation ───────────────────────────────────────────────────────

/// Validate a pipeline definition: check for missing dependencies and cycles.
/// Type-level validation is handled by the enum — no invalid type strings possible.
pub fn validate_pipeline(stages: &PipelineStages) -> anyhow::Result<()> {
    if stages.is_empty() {
        anyhow::bail!("pipeline must have at least one stage");
    }

    let ids: HashSet<&str> = stages.keys().map(|s| s.as_str()).collect();

    for (id, def) in stages {
        for dep in &def.depends_on {
            if !ids.contains(dep.as_str()) {
                anyhow::bail!("stage '{id}' depends on '{dep}' which does not exist");
            }
            if dep == id {
                anyhow::bail!("stage '{id}' depends on itself");
            }
        }
    }

    // Cycle detection via topological sort (Kahn's algorithm)
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for (id, def) in stages {
        in_degree.entry(id.as_str()).or_insert(0);
        for dep in &def.depends_on {
            adj.entry(dep.as_str()).or_default().push(id.as_str());
            *in_degree.entry(id.as_str()).or_insert(0) += 1;
        }
    }

    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(&id, _)| id)
        .collect();

    let mut visited = 0;
    while let Some(node) = queue.pop_front() {
        visited += 1;
        if let Some(neighbors) = adj.get(node) {
            for &n in neighbors {
                let d = in_degree.get_mut(n).unwrap();
                *d -= 1;
                if *d == 0 {
                    queue.push_back(n);
                }
            }
        }
    }

    if visited != stages.len() {
        anyhow::bail!("pipeline contains a cycle");
    }

    Ok(())
}

/// Find all root stages (no dependencies) — these start immediately.
pub fn find_ready_stages(stages: &PipelineStages, states: &StageStates) -> Vec<String> {
    let mut ready = Vec::new();
    for (id, def) in stages {
        let state = states.get(id);
        let is_pending = state.is_none_or(|s| s.status == StageStatus::Pending);
        if !is_pending {
            continue;
        }

        let all_deps_succeeded = def.depends_on.iter().all(|dep| {
            states
                .get(dep)
                .is_some_and(|s| s.status == StageStatus::Succeeded)
        });

        if all_deps_succeeded {
            ready.push(id.clone());
        }
    }
    ready
}

/// Check if any dependency of a stage has failed/cancelled.
pub fn has_failed_dependency(
    stage_id: &str,
    stages: &PipelineStages,
    states: &StageStates,
) -> bool {
    let Some(def) = stages.get(stage_id) else {
        return false;
    };
    def.depends_on.iter().any(|dep| {
        states.get(dep).is_some_and(|s| {
            matches!(s.status, StageStatus::Failed | StageStatus::Cancelled)
        })
    })
}

/// Check if the entire pipeline is finished (no PENDING or ACTIVE stages).
pub fn is_pipeline_complete(states: &StageStates) -> bool {
    states.values().all(|s| {
        matches!(
            s.status,
            StageStatus::Succeeded | StageStatus::Failed | StageStatus::Cancelled
        )
    })
}

/// Initialize stage_states from a pipeline definition: all PENDING.
pub fn init_stage_states(stages: &PipelineStages) -> StageStates {
    stages
        .keys()
        .map(|id| (id.clone(), StageState::pending()))
        .collect()
}

// ── CRUD service ─────────────────────────────────────────────────────────

pub struct PipelineRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub stages: serde_json::Value,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl PipelineRecord {
    /// Deserialize the stored JSON back into typed stages.
    pub fn parse_stages(&self) -> anyhow::Result<PipelineStages> {
        serde_json::from_value(self.stages.clone()).context("parse pipeline stages from DB")
    }
}

pub struct CreatePipelineParams {
    pub project_id: Uuid,
    pub name: String,
    pub stages: PipelineStages,
}

pub struct UpdatePipelineParams {
    pub enabled: Option<bool>,
    pub stages: Option<PipelineStages>,
}

#[derive(Clone)]
pub struct ReleasePipelineRegistry {
    db: PgPool,
}

impl ReleasePipelineRegistry {
    pub async fn create(&self, params: CreatePipelineParams) -> anyhow::Result<PipelineRecord> {
        validate_pipeline(&params.stages)?;

        let stages_json = serde_json::to_value(&params.stages)?;

        let rec = sqlx::query_as!(
            PipelineRecord,
            r#"INSERT INTO release_pipelines (project_id, name, stages)
            VALUES ($1, $2, $3)
            RETURNING id, project_id, name, stages, enabled, created_at, updated_at"#,
            params.project_id,
            params.name,
            stages_json,
        )
        .fetch_one(&self.db)
        .await
        .context("create release pipeline")?;

        Ok(rec)
    }

    pub async fn update(
        &self,
        project_id: &Uuid,
        name: &str,
        params: UpdatePipelineParams,
    ) -> anyhow::Result<PipelineRecord> {
        let stages_json = if let Some(ref stages) = params.stages {
            validate_pipeline(stages)?;
            Some(serde_json::to_value(stages)?)
        } else {
            None
        };

        let rec = sqlx::query_as!(
            PipelineRecord,
            r#"UPDATE release_pipelines SET
                enabled = COALESCE($3, enabled),
                stages = COALESCE($4, stages),
                updated_at = now()
            WHERE project_id = $1 AND name = $2
            RETURNING id, project_id, name, stages, enabled, created_at, updated_at"#,
            project_id,
            name,
            params.enabled,
            stages_json,
        )
        .fetch_optional(&self.db)
        .await
        .context("update release pipeline")?
        .context("release pipeline not found")?;

        Ok(rec)
    }

    pub async fn delete(&self, project_id: &Uuid, name: &str) -> anyhow::Result<()> {
        let res = sqlx::query!(
            "DELETE FROM release_pipelines WHERE project_id = $1 AND name = $2",
            project_id,
            name,
        )
        .execute(&self.db)
        .await
        .context("delete release pipeline")?;

        if res.rows_affected() != 1 {
            anyhow::bail!("release pipeline not found");
        }

        Ok(())
    }

    pub async fn list(&self, project_id: &Uuid) -> anyhow::Result<Vec<PipelineRecord>> {
        let recs = sqlx::query_as!(
            PipelineRecord,
            r#"SELECT id, project_id, name, stages, enabled, created_at, updated_at
            FROM release_pipelines
            WHERE project_id = $1
            ORDER BY name"#,
            project_id,
        )
        .fetch_all(&self.db)
        .await
        .context("list release pipelines")?;

        Ok(recs)
    }

    pub async fn get_by_name(
        &self,
        project_id: &Uuid,
        name: &str,
    ) -> anyhow::Result<Option<PipelineRecord>> {
        let rec = sqlx::query_as!(
            PipelineRecord,
            r#"SELECT id, project_id, name, stages, enabled, created_at, updated_at
            FROM release_pipelines
            WHERE project_id = $1 AND name = $2"#,
            project_id,
            name,
        )
        .fetch_optional(&self.db)
        .await
        .context("get release pipeline")?;

        Ok(rec)
    }

    /// Get the first enabled pipeline for a project.
    pub async fn get_enabled_for_project(
        &self,
        project_id: &Uuid,
    ) -> anyhow::Result<Option<PipelineRecord>> {
        let rec = sqlx::query_as!(
            PipelineRecord,
            r#"SELECT id, project_id, name, stages, enabled, created_at, updated_at
            FROM release_pipelines
            WHERE project_id = $1 AND enabled = true
            ORDER BY name
            LIMIT 1"#,
            project_id,
        )
        .fetch_optional(&self.db)
        .await
        .context("get enabled pipeline")?;

        Ok(rec)
    }
}

pub trait ReleasePipelineRegistryState {
    fn release_pipeline_registry(&self) -> ReleasePipelineRegistry;
}

impl ReleasePipelineRegistryState for State {
    fn release_pipeline_registry(&self) -> ReleasePipelineRegistry {
        ReleasePipelineRegistry {
            db: self.db.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_pipeline_simple() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "deploy-dev".into(),
            StageDefinition::deploy("dev", vec![]),
        );
        stages.insert(
            "deploy-prod".into(),
            StageDefinition::deploy("prod", vec!["deploy-dev".into()]),
        );
        assert!(validate_pipeline(&stages).is_ok());
    }

    #[test]
    fn test_validate_pipeline_cycle() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "a".into(),
            StageDefinition::deploy("dev", vec!["b".into()]),
        );
        stages.insert(
            "b".into(),
            StageDefinition::deploy("prod", vec!["a".into()]),
        );
        let err = validate_pipeline(&stages).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn test_validate_pipeline_missing_dep() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "deploy".into(),
            StageDefinition::deploy("dev", vec!["nonexistent".into()]),
        );
        let err = validate_pipeline(&stages).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn test_find_ready_stages() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "deploy-dev".into(),
            StageDefinition::deploy("dev", vec![]),
        );
        stages.insert(
            "soak".into(),
            StageDefinition::wait(300, vec!["deploy-dev".into()]),
        );
        stages.insert(
            "deploy-prod".into(),
            StageDefinition::deploy("prod", vec!["soak".into()]),
        );

        // All pending: only root should be ready
        let states = init_stage_states(&stages);
        let ready = find_ready_stages(&stages, &states);
        assert_eq!(ready, vec!["deploy-dev"]);

        // After deploy-dev succeeds, soak should be ready
        let mut states = states;
        states.get_mut("deploy-dev").unwrap().status = StageStatus::Succeeded;
        let ready = find_ready_stages(&stages, &states);
        assert_eq!(ready, vec!["soak"]);

        // After soak succeeds, deploy-prod should be ready
        states.get_mut("soak").unwrap().status = StageStatus::Succeeded;
        let ready = find_ready_stages(&stages, &states);
        assert_eq!(ready, vec!["deploy-prod"]);
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "deploy-dev".into(),
            StageDefinition::deploy("dev", vec![]),
        );
        stages.insert(
            "soak".into(),
            StageDefinition::wait(300, vec!["deploy-dev".into()]),
        );

        let json = serde_json::to_string_pretty(&stages).unwrap();
        let parsed: PipelineStages = serde_json::from_str(&json).unwrap();

        assert_eq!(stages.len(), parsed.len());
        match &parsed["deploy-dev"].config {
            StageConfig::Deploy { environment } => assert_eq!(environment, "dev"),
            _ => panic!("expected deploy stage"),
        }
        match &parsed["soak"].config {
            StageConfig::Wait { duration_seconds } => assert_eq!(*duration_seconds, 300),
            _ => panic!("expected wait stage"),
        }
    }

    #[test]
    fn test_backward_compat_json() {
        // Old-format JSON should still deserialize correctly
        let json = r#"{
            "deploy-dev": {
                "type": "deploy",
                "depends_on": [],
                "environment": "dev"
            },
            "soak": {
                "type": "wait",
                "depends_on": ["deploy-dev"],
                "duration_seconds": 300
            }
        }"#;

        let stages: PipelineStages = serde_json::from_str(json).unwrap();
        assert_eq!(stages.len(), 2);
        assert!(matches!(stages["deploy-dev"].config, StageConfig::Deploy { .. }));
        assert!(matches!(stages["soak"].config, StageConfig::Wait { .. }));
    }

    #[test]
    fn test_plan_stage_serde() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "plan-prod".into(),
            StageDefinition::plan("prod", false, vec![]),
        );
        stages.insert(
            "deploy-prod".into(),
            StageDefinition::deploy("prod", vec!["plan-prod".into()]),
        );

        let json = serde_json::to_string_pretty(&stages).unwrap();
        let parsed: PipelineStages = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.len(), 2);
        match &parsed["plan-prod"].config {
            StageConfig::Plan { environment, auto_approve } => {
                assert_eq!(environment, "prod");
                assert!(!auto_approve);
            }
            _ => panic!("expected plan stage"),
        }
    }

    #[test]
    fn test_plan_stage_auto_approve_serde() {
        let json = r#"{
            "plan-prod": {
                "type": "plan",
                "depends_on": [],
                "environment": "prod",
                "auto_approve": true
            }
        }"#;

        let stages: PipelineStages = serde_json::from_str(json).unwrap();
        match &stages["plan-prod"].config {
            StageConfig::Plan { environment, auto_approve } => {
                assert_eq!(environment, "prod");
                assert!(auto_approve);
            }
            _ => panic!("expected plan stage"),
        }
    }

    #[test]
    fn test_plan_stage_auto_approve_defaults_false() {
        let json = r#"{
            "plan-prod": {
                "type": "plan",
                "depends_on": [],
                "environment": "prod"
            }
        }"#;

        let stages: PipelineStages = serde_json::from_str(json).unwrap();
        match &stages["plan-prod"].config {
            StageConfig::Plan { auto_approve, .. } => {
                assert!(!auto_approve);
            }
            _ => panic!("expected plan stage"),
        }
    }

    #[test]
    fn test_plan_then_deploy_pipeline() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "plan-prod".into(),
            StageDefinition::plan("prod", false, vec![]),
        );
        stages.insert(
            "deploy-prod".into(),
            StageDefinition::deploy("prod", vec!["plan-prod".into()]),
        );

        assert!(validate_pipeline(&stages).is_ok());

        let states = init_stage_states(&stages);
        let ready = find_ready_stages(&stages, &states);
        assert_eq!(ready, vec!["plan-prod"]);

        // After plan succeeds, deploy should be ready
        let mut states = states;
        states.get_mut("plan-prod").unwrap().status = StageStatus::Succeeded;
        let ready = find_ready_stages(&stages, &states);
        assert_eq!(ready, vec!["deploy-prod"]);
    }

    #[test]
    fn test_approval_status_serde() {
        let state = StageState {
            status: StageStatus::Active,
            approval_status: Some(ApprovalStatus::AwaitingApproval),
            ..StageState::pending()
        };

        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("AWAITING_APPROVAL"));

        let parsed: StageState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.approval_status, Some(ApprovalStatus::AwaitingApproval));
    }
}
