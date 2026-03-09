use anyhow::Context;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::State;

#[derive(Clone)]
pub struct PolicyRegistry {
    db: PgPool,
}

// ── Database record ─────────────────────────────────────────────────

pub struct PolicyRecord {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub policy_type: String,
    pub config: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

// ── Domain types ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyType {
    SoakTime,
    BranchRestriction,
}

impl PolicyType {
    pub fn as_str(&self) -> &'static str {
        match self {
            PolicyType::SoakTime => "soak_time",
            PolicyType::BranchRestriction => "branch_restriction",
        }
    }
}

impl std::str::FromStr for PolicyType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "soak_time" => Ok(PolicyType::SoakTime),
            "branch_restriction" => Ok(PolicyType::BranchRestriction),
            other => anyhow::bail!("unknown policy type: {other}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoakTimeConfig {
    pub source_environment: String,
    pub target_environment: String,
    pub duration_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchRestrictionConfig {
    pub target_environment: String,
    pub branch_pattern: String,
}

#[derive(Debug, Clone)]
pub enum PolicyConfig {
    SoakTime(SoakTimeConfig),
    BranchRestriction(BranchRestrictionConfig),
}

impl PolicyConfig {
    pub fn policy_type(&self) -> PolicyType {
        match self {
            PolicyConfig::SoakTime(_) => PolicyType::SoakTime,
            PolicyConfig::BranchRestriction(_) => PolicyType::BranchRestriction,
        }
    }

    pub fn to_json(&self) -> anyhow::Result<serde_json::Value> {
        match self {
            PolicyConfig::SoakTime(c) => {
                serde_json::to_value(c).context("serialize soak_time config")
            }
            PolicyConfig::BranchRestriction(c) => {
                serde_json::to_value(c).context("serialize branch_restriction config")
            }
        }
    }

    pub fn from_record(policy_type: &str, config: &serde_json::Value) -> anyhow::Result<Self> {
        match policy_type {
            "soak_time" => {
                let c: SoakTimeConfig =
                    serde_json::from_value(config.clone()).context("parse soak_time config")?;
                Ok(PolicyConfig::SoakTime(c))
            }
            "branch_restriction" => {
                let c: BranchRestrictionConfig = serde_json::from_value(config.clone())
                    .context("parse branch_restriction config")?;
                Ok(PolicyConfig::BranchRestriction(c))
            }
            other => anyhow::bail!("unknown policy type: {other}"),
        }
    }
}

// ── Evaluation result ───────────────────────────────────────────────

#[derive(Debug)]
pub struct PolicyEvaluation {
    pub policy_name: String,
    pub policy_type: PolicyType,
    pub passed: bool,
    pub reason: String,
}

// ── CRUD params ─────────────────────────────────────────────────────

pub struct CreatePolicyParams {
    pub project_id: Uuid,
    pub name: String,
    pub config: PolicyConfig,
}

pub struct UpdatePolicyParams {
    pub enabled: Option<bool>,
    pub config: Option<PolicyConfig>,
}

// ── Implementation ──────────────────────────────────────────────────

impl PolicyRegistry {
    pub async fn create(&self, params: CreatePolicyParams) -> anyhow::Result<PolicyRecord> {
        // Validate config
        self.validate_config(&params.config)?;

        let policy_type = params.config.policy_type().as_str().to_string();
        let config_json = params.config.to_json()?;

        let rec = sqlx::query_as!(
            PolicyRecord,
            r#"INSERT INTO policies (project_id, name, policy_type, config)
            VALUES ($1, $2, $3, $4)
            RETURNING id, project_id, name, enabled, policy_type, config, created_at, updated_at"#,
            params.project_id,
            params.name,
            policy_type,
            config_json,
        )
        .fetch_one(&self.db)
        .await
        .context("create policy")?;

        Ok(rec)
    }

    pub async fn update(
        &self,
        project_id: &Uuid,
        name: &str,
        params: UpdatePolicyParams,
    ) -> anyhow::Result<PolicyRecord> {
        if let Some(ref config) = params.config {
            self.validate_config(config)?;
        }

        let (policy_type, config_json) = match params.config {
            Some(ref c) => (Some(c.policy_type().as_str().to_string()), Some(c.to_json()?)),
            None => (None, None),
        };

        let rec = sqlx::query_as!(
            PolicyRecord,
            r#"UPDATE policies SET
                enabled = COALESCE($3, enabled),
                policy_type = COALESCE($4, policy_type),
                config = COALESCE($5, config),
                updated_at = now()
            WHERE project_id = $1 AND name = $2
            RETURNING id, project_id, name, enabled, policy_type, config, created_at, updated_at"#,
            project_id,
            name,
            params.enabled,
            policy_type,
            config_json,
        )
        .fetch_optional(&self.db)
        .await
        .context("update policy")?
        .context("policy not found")?;

        Ok(rec)
    }

    pub async fn delete(&self, project_id: &Uuid, name: &str) -> anyhow::Result<()> {
        let res = sqlx::query!(
            "DELETE FROM policies WHERE project_id = $1 AND name = $2",
            project_id,
            name,
        )
        .execute(&self.db)
        .await
        .context("delete policy")?;

        if res.rows_affected() != 1 {
            anyhow::bail!("policy not found");
        }

        Ok(())
    }

    pub async fn list(&self, project_id: &Uuid) -> anyhow::Result<Vec<PolicyRecord>> {
        let recs = sqlx::query_as!(
            PolicyRecord,
            r#"SELECT id, project_id, name, enabled, policy_type, config, created_at, updated_at
            FROM policies
            WHERE project_id = $1
            ORDER BY name"#,
            project_id,
        )
        .fetch_all(&self.db)
        .await
        .context("list policies")?;

        Ok(recs)
    }

    /// Evaluate all enabled policies for a project against a target environment.
    /// Returns evaluation results for each relevant policy.
    pub async fn evaluate_for_environment(
        &self,
        project_id: &Uuid,
        target_environment: &str,
        branch: Option<&str>,
    ) -> anyhow::Result<Vec<PolicyEvaluation>> {
        let policies = sqlx::query_as!(
            PolicyRecord,
            r#"SELECT id, project_id, name, enabled, policy_type, config, created_at, updated_at
            FROM policies
            WHERE project_id = $1 AND enabled = true
            ORDER BY name"#,
            project_id,
        )
        .fetch_all(&self.db)
        .await
        .context("evaluate policies")?;

        let mut evaluations = Vec::new();

        for policy in policies {
            let config = PolicyConfig::from_record(&policy.policy_type, &policy.config)?;

            match config {
                PolicyConfig::SoakTime(ref c) => {
                    if c.target_environment != target_environment {
                        continue;
                    }
                    let eval = self
                        .check_soak_time(project_id, c, &policy.name)
                        .await?;
                    evaluations.push(eval);
                }
                PolicyConfig::BranchRestriction(ref c) => {
                    if c.target_environment != target_environment {
                        continue;
                    }
                    let eval = self.check_branch_restriction(c, branch, &policy.name);
                    evaluations.push(eval);
                }
            }
        }

        Ok(evaluations)
    }

    // ── Internal helpers ────────────────────────────────────────────

    fn validate_config(&self, config: &PolicyConfig) -> anyhow::Result<()> {
        match config {
            PolicyConfig::SoakTime(c) => {
                if c.source_environment.is_empty() {
                    anyhow::bail!("source_environment is required for soak_time policy");
                }
                if c.target_environment.is_empty() {
                    anyhow::bail!("target_environment is required for soak_time policy");
                }
                if c.duration_seconds <= 0 {
                    anyhow::bail!("duration_seconds must be positive for soak_time policy");
                }
            }
            PolicyConfig::BranchRestriction(c) => {
                if c.target_environment.is_empty() {
                    anyhow::bail!("target_environment is required for branch_restriction policy");
                }
                if c.branch_pattern.is_empty() {
                    anyhow::bail!("branch_pattern is required for branch_restriction policy");
                }
                Regex::new(&c.branch_pattern)
                    .context("invalid regex for branch_pattern")?;
            }
        }
        Ok(())
    }

    async fn check_soak_time(
        &self,
        project_id: &Uuid,
        config: &SoakTimeConfig,
        policy_name: &str,
    ) -> anyhow::Result<PolicyEvaluation> {
        // Find the most recent successful release to the source environment
        let last_success = sqlx::query_scalar!(
            r#"SELECT MAX(rs.updated_at) as "max_updated_at"
            FROM release_states rs
            JOIN destinations d ON rs.destination_id = d.id
            WHERE rs.project_id = $1
              AND d.environment = $2
              AND rs.status = 'SUCCEEDED'"#,
            project_id,
            config.source_environment,
        )
        .fetch_one(&self.db)
        .await
        .context("check soak time")?;

        match last_success {
            Some(ts) => {
                let elapsed = chrono::Utc::now() - ts;
                let required = chrono::Duration::seconds(config.duration_seconds);

                if elapsed >= required {
                    Ok(PolicyEvaluation {
                        policy_name: policy_name.to_string(),
                        policy_type: PolicyType::SoakTime,
                        passed: true,
                        reason: format!(
                            "soak time satisfied: {}s elapsed since last {} deploy (required: {}s)",
                            elapsed.num_seconds(),
                            config.source_environment,
                            config.duration_seconds,
                        ),
                    })
                } else {
                    let remaining = (required - elapsed).num_seconds();
                    Ok(PolicyEvaluation {
                        policy_name: policy_name.to_string(),
                        policy_type: PolicyType::SoakTime,
                        passed: false,
                        reason: format!(
                            "soak time not met: {}s remaining ({}s elapsed, {}s required after {} deploy)",
                            remaining,
                            elapsed.num_seconds(),
                            config.duration_seconds,
                            config.source_environment,
                        ),
                    })
                }
            }
            None => Ok(PolicyEvaluation {
                policy_name: policy_name.to_string(),
                policy_type: PolicyType::SoakTime,
                passed: true,
                reason: format!(
                    "no successful deploy to {} found yet — soak time not applicable",
                    config.source_environment,
                ),
            }),
        }
    }

    fn check_branch_restriction(
        &self,
        config: &BranchRestrictionConfig,
        branch: Option<&str>,
        policy_name: &str,
    ) -> PolicyEvaluation {
        match branch {
            Some(b) => match Regex::new(&config.branch_pattern) {
                Ok(re) => {
                    if re.is_match(b) {
                        PolicyEvaluation {
                            policy_name: policy_name.to_string(),
                            policy_type: PolicyType::BranchRestriction,
                            passed: true,
                            reason: format!(
                                "branch '{}' matches pattern '{}'",
                                b, config.branch_pattern
                            ),
                        }
                    } else {
                        PolicyEvaluation {
                            policy_name: policy_name.to_string(),
                            policy_type: PolicyType::BranchRestriction,
                            passed: false,
                            reason: format!(
                                "branch '{}' does not match required pattern '{}' for {}",
                                b, config.branch_pattern, config.target_environment
                            ),
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        pattern = config.branch_pattern,
                        "invalid regex in branch restriction policy: {e}"
                    );
                    PolicyEvaluation {
                        policy_name: policy_name.to_string(),
                        policy_type: PolicyType::BranchRestriction,
                        passed: false,
                        reason: format!("invalid branch pattern: {e}"),
                    }
                }
            },
            None => PolicyEvaluation {
                policy_name: policy_name.to_string(),
                policy_type: PolicyType::BranchRestriction,
                passed: true,
                reason: format!(
                    "no branch information available — skipping branch restriction for {}",
                    config.target_environment
                ),
            },
        }
    }
}

pub trait PolicyRegistryState {
    fn policy_registry(&self) -> PolicyRegistry;
}

impl PolicyRegistryState for State {
    fn policy_registry(&self) -> PolicyRegistry {
        PolicyRegistry {
            db: self.db.clone(),
        }
    }
}
