use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::State;

pub type PipelineStages = HashMap<String, StageDefinition>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageDefinition {
    #[serde(default)]
    pub depends_on: Vec<String>,

    #[serde(flatten)]
    pub config: StageConfig,
}

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
        #[serde(default)]
        auto_approve: bool,
        #[serde(default)]
        destroy: bool,
    },
    Gate {
        requires: Vec<SignalRequirement>,
        timeout_seconds: i64,
        #[serde(default)]
        on_timeout: GateTimeout,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalRequirement {
    pub signal: String,

    #[serde(default, rename = "in")]
    pub accept: Vec<String>,
}

impl SignalRequirement {
    pub fn accepted(&self) -> Vec<String> {
        if self.accept.is_empty() {
            vec!["HEALTHY".to_string()]
        } else {
            self.accept.clone()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateTimeout {
    #[default]
    Fail,
    Proceed,
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

    pub fn plan(
        environment: impl Into<String>,
        auto_approve: bool,
        depends_on: Vec<String>,
    ) -> Self {
        Self {
            depends_on,
            config: StageConfig::Plan {
                environment: environment.into(),
                auto_approve,
                destroy: false,
            },
        }
    }

    pub fn destroy_plan(environment: impl Into<String>, depends_on: Vec<String>) -> Self {
        Self {
            depends_on,
            config: StageConfig::Plan {
                environment: environment.into(),
                auto_approve: false,
                destroy: true,
            },
        }
    }

    pub fn gate(
        requires: Vec<SignalRequirement>,
        timeout_seconds: i64,
        on_timeout: GateTimeout,
        depends_on: Vec<String>,
    ) -> Self {
        Self {
            depends_on,
            config: StageConfig::Gate {
                requires,
                timeout_seconds,
                on_timeout,
            },
        }
    }
}

pub type StageStates = HashMap<String, StageState>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageState {
    pub status: StageStatus,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_ids: Option<Vec<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_until: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_status: Option<ApprovalStatus>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_deadline: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_waiting_on: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApprovalStatus {
    AwaitingApproval,
    Approved,
    Rejected,
}

impl ApprovalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AwaitingApproval => "AWAITING_APPROVAL",
            Self::Approved => "APPROVED",
            Self::Rejected => "REJECTED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StageStatus {
    Pending,
    Active,
    Succeeded,
    Failed,
    Cancelled,
    Superseded,
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
            gate_deadline: None,
            gate_waiting_on: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageType {
    Deploy,
    Wait,
    Plan,
    Gate,
}

impl StageType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Deploy => "deploy",
            Self::Wait => "wait",
            Self::Plan => "plan",
            Self::Gate => "gate",
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
            Self::Gate { .. } => StageType::Gate,
        }
    }
}

pub fn validate_pipeline(stages: &PipelineStages) -> anyhow::Result<()> {
    if stages.is_empty() {
        anyhow::bail!("pipeline must have at least one stage");
    }

    let ids: HashSet<&str> = stages.keys().map(|s| s.as_str()).collect();

    for (id, def) in stages {
        if let StageConfig::Plan {
            destroy: true,
            auto_approve: true,
            ..
        } = &def.config
        {
            anyhow::bail!(
                "stage '{id}' is a destroy plan with auto_approve — a destroy must be approved by a person"
            );
        }
    }

    for (id, def) in stages {
        if let StageConfig::Gate {
            requires,
            timeout_seconds,
            ..
        } = &def.config
        {
            if requires.is_empty() {
                anyhow::bail!(
                    "gate stage '{id}' requires no signals, so it would open the moment it \
                     is reached — give it a requirement or use a wait stage"
                );
            }
            if *timeout_seconds <= 0 {
                anyhow::bail!(
                    "gate stage '{id}' has timeout_seconds {timeout_seconds}; a gate that \
                     never times out blocks the pipeline with nothing reporting why"
                );
            }
            for req in requires {
                if req.signal.trim().is_empty() {
                    anyhow::bail!("gate stage '{id}' has a requirement with no signal name");
                }
                for status in req.accepted() {
                    if !crate::services::release_signals::is_valid_status(&status) {
                        anyhow::bail!(
                            "gate stage '{id}' waits for signal '{}' to be '{status}', which \
                             is not a status any reporter can send — expected one of {:?}",
                            req.signal,
                            crate::services::release_signals::VALID_STATUSES,
                        );
                    }
                }
            }
        }

        for dep in &def.depends_on {
            if !ids.contains(dep.as_str()) {
                anyhow::bail!("stage '{id}' depends on '{dep}' which does not exist");
            }
            if dep == id {
                anyhow::bail!("stage '{id}' depends on itself");
            }
        }
    }

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

pub fn has_failed_dependency(
    stage_id: &str,
    stages: &PipelineStages,
    states: &StageStates,
) -> bool {
    let Some(def) = stages.get(stage_id) else {
        return false;
    };
    def.depends_on.iter().any(|dep| {
        states
            .get(dep)
            .is_some_and(|s| matches!(s.status, StageStatus::Failed | StageStatus::Cancelled))
    })
}

pub fn has_superseded_dependency(
    stage_id: &str,
    stages: &PipelineStages,
    states: &StageStates,
) -> bool {
    let Some(def) = stages.get(stage_id) else {
        return false;
    };
    def.depends_on.iter().any(|dep| {
        states
            .get(dep)
            .is_some_and(|s| s.status == StageStatus::Superseded)
    })
}

pub fn is_pipeline_complete(states: &StageStates) -> bool {
    states.values().all(|s| {
        matches!(
            s.status,
            StageStatus::Succeeded
                | StageStatus::Failed
                | StageStatus::Cancelled
                | StageStatus::Superseded
        )
    })
}

pub fn init_stage_states(stages: &PipelineStages) -> StageStates {
    stages
        .keys()
        .map(|id| (id.clone(), StageState::pending()))
        .collect()
}

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
    fn approval_status_wire_spelling_is_screaming_snake_case() {
        assert_eq!(
            ApprovalStatus::AwaitingApproval.as_str(),
            "AWAITING_APPROVAL"
        );
        assert_eq!(ApprovalStatus::Approved.as_str(), "APPROVED");
        assert_eq!(ApprovalStatus::Rejected.as_str(), "REJECTED");

        for status in [
            ApprovalStatus::AwaitingApproval,
            ApprovalStatus::Approved,
            ApprovalStatus::Rejected,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(json.trim_matches('"'), status.as_str());
        }
    }

    #[test]
    fn test_validate_pipeline_simple() {
        let mut stages = PipelineStages::new();
        stages.insert("deploy-dev".into(), StageDefinition::deploy("dev", vec![]));
        stages.insert(
            "deploy-prod".into(),
            StageDefinition::deploy("prod", vec!["deploy-dev".into()]),
        );
        assert!(validate_pipeline(&stages).is_ok());
    }

    #[test]
    fn test_validate_pipeline_cycle() {
        let mut stages = PipelineStages::new();
        stages.insert("a".into(), StageDefinition::deploy("dev", vec!["b".into()]));
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
        stages.insert("deploy-dev".into(), StageDefinition::deploy("dev", vec![]));
        stages.insert(
            "soak".into(),
            StageDefinition::wait(300, vec!["deploy-dev".into()]),
        );
        stages.insert(
            "deploy-prod".into(),
            StageDefinition::deploy("prod", vec!["soak".into()]),
        );

        let states = init_stage_states(&stages);
        let ready = find_ready_stages(&stages, &states);
        assert_eq!(ready, vec!["deploy-dev"]);

        let mut states = states;
        states.get_mut("deploy-dev").unwrap().status = StageStatus::Succeeded;
        let ready = find_ready_stages(&stages, &states);
        assert_eq!(ready, vec!["soak"]);

        states.get_mut("soak").unwrap().status = StageStatus::Succeeded;
        let ready = find_ready_stages(&stages, &states);
        assert_eq!(ready, vec!["deploy-prod"]);
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut stages = PipelineStages::new();
        stages.insert("deploy-dev".into(), StageDefinition::deploy("dev", vec![]));
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
        assert!(matches!(
            stages["deploy-dev"].config,
            StageConfig::Deploy { .. }
        ));
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
            StageConfig::Plan {
                environment,
                auto_approve,
                ..
            } => {
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
            StageConfig::Plan {
                environment,
                auto_approve,
                ..
            } => {
                assert_eq!(environment, "prod");
                assert!(auto_approve);
            }
            _ => panic!("expected plan stage"),
        }
    }

    #[test]
    fn a_destroy_plan_that_approves_itself_is_refused() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "destroy-dev".into(),
            StageDefinition {
                depends_on: vec![],
                config: StageConfig::Plan {
                    environment: "dev".into(),
                    auto_approve: true,
                    destroy: true,
                },
            },
        );
        let err = validate_pipeline(&stages).expect_err("auto-approved destroy");
        assert!(err.to_string().contains("approved by a person"), "{err}");
    }

    #[test]
    fn a_gated_destroy_plan_followed_by_a_deploy_is_valid() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "destroy-dev".into(),
            StageDefinition::destroy_plan("dev", vec![]),
        );
        stages.insert(
            "apply-dev".into(),
            StageDefinition::deploy("dev", vec!["destroy-dev".into()]),
        );
        validate_pipeline(&stages).expect("valid");
    }

    #[test]
    fn destroy_defaults_false_so_existing_pipelines_are_unchanged() {
        let json = r#"{"plan-prod": {"type": "plan", "depends_on": [], "environment": "prod"}}"#;
        let stages: PipelineStages = serde_json::from_str(json).unwrap();
        assert!(matches!(
            stages["plan-prod"].config,
            StageConfig::Plan { destroy: false, .. }
        ));
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
        assert_eq!(
            parsed.approval_status,
            Some(ApprovalStatus::AwaitingApproval)
        );
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    fn req(signal: &str, accept: &[&str]) -> SignalRequirement {
        SignalRequirement {
            signal: signal.to_string(),
            accept: accept.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn gate_pipeline(requires: Vec<SignalRequirement>, timeout_seconds: i64) -> PipelineStages {
        let mut stages = PipelineStages::new();
        stages.insert(
            "deploy-demo".into(),
            StageDefinition::deploy("demo", vec![]),
        );
        stages.insert(
            "await-demo".into(),
            StageDefinition::gate(
                requires,
                timeout_seconds,
                GateTimeout::Fail,
                vec!["deploy-demo".into()],
            ),
        );
        stages
    }

    #[test]
    fn a_well_formed_gate_validates() {
        let stages = gate_pipeline(vec![req("rollout", &["HEALTHY"])], 600);
        validate_pipeline(&stages).expect("should validate");
    }

    #[test]
    fn a_gate_with_no_requirements_is_rejected() {
        let stages = gate_pipeline(vec![], 600);
        let err = validate_pipeline(&stages).unwrap_err().to_string();
        assert!(err.contains("requires no signals"), "got: {err}");
    }

    #[test]
    fn a_gate_without_a_timeout_is_rejected() {
        for timeout in [0, -1] {
            let stages = gate_pipeline(vec![req("rollout", &["HEALTHY"])], timeout);
            let err = validate_pipeline(&stages).unwrap_err().to_string();
            assert!(err.contains("never times out"), "got: {err}");
        }
    }

    #[test]
    fn a_gate_waiting_for_an_impossible_status_is_rejected() {
        let stages = gate_pipeline(vec![req("rollout", &["HEALTY"])], 600);
        let err = validate_pipeline(&stages).unwrap_err().to_string();
        assert!(
            err.contains("not a status any reporter can send"),
            "got: {err}"
        );
        assert!(
            err.contains("HEALTY"),
            "should name the offending value: {err}"
        );
    }

    #[test]
    fn a_requirement_with_no_signal_name_is_rejected() {
        let stages = gate_pipeline(vec![req("  ", &["HEALTHY"])], 600);
        let err = validate_pipeline(&stages).unwrap_err().to_string();
        assert!(err.contains("no signal name"), "got: {err}");
    }

    #[test]
    fn an_omitted_accept_list_means_healthy() {
        assert_eq!(req("rollout", &[]).accepted(), vec!["HEALTHY".to_string()]);
        let stages = gate_pipeline(vec![req("rollout", &[])], 600);
        validate_pipeline(&stages).expect("should validate");
    }

    #[test]
    fn on_timeout_defaults_to_fail() {
        assert_eq!(GateTimeout::default(), GateTimeout::Fail);
    }

    #[test]
    fn a_gate_round_trips_through_the_stored_json() {
        let json = r#"{
            "deploy-demo": {"type": "deploy", "environment": "platform-dev"},
            "await-demo": {
                "type": "gate",
                "requires": [{"signal": "rollout", "in": ["HEALTHY"]}],
                "timeout_seconds": 600,
                "depends_on": ["deploy-demo"]
            },
            "deploy-finance": {
                "type": "deploy", "environment": "finance",
                "depends_on": ["await-demo"]
            }
        }"#;
        let stages: PipelineStages = serde_json::from_str(json).expect("parses");
        validate_pipeline(&stages).expect("validates");

        let gate = &stages["await-demo"];
        assert_eq!(gate.depends_on, vec!["deploy-demo".to_string()]);
        assert_eq!(gate.config.stage_type(), StageType::Gate);
        match &gate.config {
            StageConfig::Gate {
                requires,
                timeout_seconds,
                on_timeout,
            } => {
                assert_eq!(requires.len(), 1);
                assert_eq!(requires[0].signal, "rollout");
                assert_eq!(requires[0].accepted(), vec!["HEALTHY".to_string()]);
                assert_eq!(*timeout_seconds, 600);
                assert_eq!(*on_timeout, GateTimeout::Fail);
            }
            other => panic!("expected a gate, got {other:?}"),
        }

        let back = serde_json::to_string(&stages).expect("serialises");
        let again: PipelineStages = serde_json::from_str(&back).expect("re-parses");
        assert_eq!(again["await-demo"].config.stage_type(), StageType::Gate);
    }

    #[test]
    fn a_stage_behind_an_unsatisfied_gate_is_not_ready() {
        let stages = {
            let mut s = gate_pipeline(vec![req("rollout", &["HEALTHY"])], 600);
            s.insert(
                "deploy-finance".into(),
                StageDefinition::deploy("finance", vec!["await-demo".into()]),
            );
            s
        };
        let mut states = init_stage_states(&stages);
        states.insert(
            "deploy-demo".into(),
            StageState {
                status: StageStatus::Succeeded,
                ..StageState::pending()
            },
        );
        states.insert(
            "await-demo".into(),
            StageState {
                status: StageStatus::Active,
                ..StageState::pending()
            },
        );

        let ready = find_ready_stages(&stages, &states);
        assert!(
            !ready.contains(&"deploy-finance".to_string()),
            "finance must wait for the gate; ready = {ready:?}"
        );

        states.insert(
            "await-demo".into(),
            StageState {
                status: StageStatus::Succeeded,
                ..StageState::pending()
            },
        );
        let ready = find_ready_stages(&stages, &states);
        assert!(
            ready.contains(&"deploy-finance".to_string()),
            "ready = {ready:?}"
        );
    }
}
