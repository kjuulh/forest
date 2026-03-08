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
    /// Stage type: "deploy", "wait", "build", "monitor", "manual"
    #[serde(rename = "type")]
    pub stage_type: String,

    /// Which stages must complete before this one starts.
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// For deploy stages: the environment name (resolved to destinations at runtime).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,

    /// For wait stages: how long to wait (seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<i64>,
}

// ── Runtime state types (stored in release_intents.stage_states) ─────────

pub type StageStates = HashMap<String, StageState>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageState {
    pub status: StageStatus,

    /// UUIDs of release_states rows created for this stage (deploy stages).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_ids: Option<Vec<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,

    /// For wait stages: ISO8601 timestamp when the wait expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_until: Option<String>,
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
            release_ids: None,
            error_message: None,
            started_at: None,
            completed_at: None,
            wait_until: None,
        }
    }
}

// ── DAG validation ───────────────────────────────────────────────────────

/// Validate a pipeline definition: check for missing dependencies, cycles,
/// and that all stages have valid types with required config.
pub fn validate_pipeline(stages: &PipelineStages) -> anyhow::Result<()> {
    if stages.is_empty() {
        anyhow::bail!("pipeline must have at least one stage");
    }

    let ids: HashSet<&str> = stages.keys().map(|s| s.as_str()).collect();

    // Check all depends_on references exist
    for (id, def) in stages {
        for dep in &def.depends_on {
            if !ids.contains(dep.as_str()) {
                anyhow::bail!("stage '{id}' depends on '{dep}' which does not exist");
            }
            if dep == id {
                anyhow::bail!("stage '{id}' depends on itself");
            }
        }

        // Validate stage type + required config
        match def.stage_type.as_str() {
            "deploy" => {
                if def.environment.is_none() {
                    anyhow::bail!("deploy stage '{id}' requires an 'environment' field");
                }
            }
            "wait" => {
                if def.duration_seconds.is_none() {
                    anyhow::bail!("wait stage '{id}' requires a 'duration_seconds' field");
                }
            }
            other => {
                anyhow::bail!("unknown stage type '{other}' for stage '{id}'");
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
        let is_pending = state.map_or(true, |s| s.status == StageStatus::Pending);
        if !is_pending {
            continue;
        }

        let all_deps_succeeded = def.depends_on.iter().all(|dep| {
            states
                .get(dep)
                .map_or(false, |s| s.status == StageStatus::Succeeded)
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
        states.get(dep).map_or(false, |s| {
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
            StageDefinition {
                stage_type: "deploy".into(),
                depends_on: vec![],
                environment: Some("dev".into()),
                duration_seconds: None,
            },
        );
        stages.insert(
            "deploy-prod".into(),
            StageDefinition {
                stage_type: "deploy".into(),
                depends_on: vec!["deploy-dev".into()],
                environment: Some("prod".into()),
                duration_seconds: None,
            },
        );
        assert!(validate_pipeline(&stages).is_ok());
    }

    #[test]
    fn test_validate_pipeline_cycle() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "a".into(),
            StageDefinition {
                stage_type: "deploy".into(),
                depends_on: vec!["b".into()],
                environment: Some("dev".into()),
                duration_seconds: None,
            },
        );
        stages.insert(
            "b".into(),
            StageDefinition {
                stage_type: "deploy".into(),
                depends_on: vec!["a".into()],
                environment: Some("prod".into()),
                duration_seconds: None,
            },
        );
        let err = validate_pipeline(&stages).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn test_validate_pipeline_missing_dep() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "deploy".into(),
            StageDefinition {
                stage_type: "deploy".into(),
                depends_on: vec!["nonexistent".into()],
                environment: Some("dev".into()),
                duration_seconds: None,
            },
        );
        let err = validate_pipeline(&stages).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn test_validate_pipeline_deploy_no_env() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "deploy".into(),
            StageDefinition {
                stage_type: "deploy".into(),
                depends_on: vec![],
                environment: None,
                duration_seconds: None,
            },
        );
        let err = validate_pipeline(&stages).unwrap_err();
        assert!(err.to_string().contains("environment"));
    }

    #[test]
    fn test_validate_pipeline_wait_no_duration() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "soak".into(),
            StageDefinition {
                stage_type: "wait".into(),
                depends_on: vec![],
                environment: None,
                duration_seconds: None,
            },
        );
        let err = validate_pipeline(&stages).unwrap_err();
        assert!(err.to_string().contains("duration_seconds"));
    }

    #[test]
    fn test_find_ready_stages() {
        let mut stages = PipelineStages::new();
        stages.insert(
            "deploy-dev".into(),
            StageDefinition {
                stage_type: "deploy".into(),
                depends_on: vec![],
                environment: Some("dev".into()),
                duration_seconds: None,
            },
        );
        stages.insert(
            "soak".into(),
            StageDefinition {
                stage_type: "wait".into(),
                depends_on: vec!["deploy-dev".into()],
                environment: None,
                duration_seconds: Some(300),
            },
        );
        stages.insert(
            "deploy-prod".into(),
            StageDefinition {
                stage_type: "deploy".into(),
                depends_on: vec!["soak".into()],
                environment: Some("prod".into()),
                duration_seconds: None,
            },
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
}
