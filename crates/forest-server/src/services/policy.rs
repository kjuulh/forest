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
    Approval,
}

impl PolicyType {
    pub fn as_str(&self) -> &'static str {
        match self {
            PolicyType::SoakTime => "soak_time",
            PolicyType::BranchRestriction => "branch_restriction",
            PolicyType::Approval => "approval",
        }
    }
}

impl std::str::FromStr for PolicyType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "soak_time" => Ok(PolicyType::SoakTime),
            "branch_restriction" => Ok(PolicyType::BranchRestriction),
            "approval" => Ok(PolicyType::Approval),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalConfig {
    pub target_environment: String,
    pub required_approvals: i32,
}

#[derive(Debug, Clone)]
pub enum PolicyConfig {
    SoakTime(SoakTimeConfig),
    BranchRestriction(BranchRestrictionConfig),
    Approval(ApprovalConfig),
}

impl PolicyConfig {
    pub fn policy_type(&self) -> PolicyType {
        match self {
            PolicyConfig::SoakTime(_) => PolicyType::SoakTime,
            PolicyConfig::BranchRestriction(_) => PolicyType::BranchRestriction,
            PolicyConfig::Approval(_) => PolicyType::Approval,
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
            PolicyConfig::Approval(c) => {
                serde_json::to_value(c).context("serialize approval config")
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
            "approval" => {
                let c: ApprovalConfig = serde_json::from_value(config.clone())
                    .context("parse approval config")?;
                Ok(PolicyConfig::Approval(c))
            }
            other => anyhow::bail!("unknown policy type: {other}"),
        }
    }
}

// ── Evaluation result ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ApprovalStateInfo {
    pub required_approvals: i32,
    pub current_approvals: i32,
    pub decisions: Vec<ApprovalDecisionInfo>,
}

#[derive(Debug, Clone)]
pub struct ApprovalDecisionInfo {
    pub user_id: String,
    pub username: String,
    pub decision: String,
    pub decided_at: String,
    pub comment: Option<String>,
}

#[derive(Debug)]
pub struct PolicyEvaluation {
    pub policy_name: String,
    pub policy_type: PolicyType,
    pub passed: bool,
    pub reason: String,
    pub approval_state: Option<ApprovalStateInfo>,
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
        release_intent_id: Option<&Uuid>,
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
                PolicyConfig::Approval(ref c) => {
                    if c.target_environment != target_environment {
                        continue;
                    }
                    let eval = self.check_approval(&policy.id, c, &policy.name, release_intent_id).await?;
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
            PolicyConfig::Approval(c) => {
                if c.target_environment.is_empty() {
                    anyhow::bail!("target_environment is required for approval policy");
                }
                if c.required_approvals < 1 {
                    anyhow::bail!("required_approvals must be >= 1 for approval policy");
                }
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
                        approval_state: None,
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
                        approval_state: None,
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
                approval_state: None,
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
                            approval_state: None,
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
                            approval_state: None,
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
                        approval_state: None,
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
                approval_state: None,
            },
        }
    }

    async fn check_approval(
        &self,
        _policy_id: &Uuid,
        config: &ApprovalConfig,
        policy_name: &str,
        release_intent_id: Option<&Uuid>,
    ) -> anyhow::Result<PolicyEvaluation> {
        let Some(intent_id) = release_intent_id else {
            return Ok(PolicyEvaluation {
                policy_name: policy_name.to_string(),
                policy_type: PolicyType::Approval,
                passed: false,
                reason: "approval required — no release intent context".to_string(),
                approval_state: Some(ApprovalStateInfo {
                    required_approvals: config.required_approvals,
                    current_approvals: 0,
                    decisions: vec![],
                }),
            });
        };

        let approved_count = sqlx::query_scalar!(
            r#"SELECT COUNT(*) as "count!" FROM approval_decisions
             WHERE release_intent_id = $1 AND target_environment = $2 AND decision = 'approved'"#,
            intent_id,
            config.target_environment,
        )
        .fetch_one(&self.db)
        .await
        .context("count approvals")?;

        let decisions = sqlx::query!(
            r#"SELECT user_id, username, decision, comment, created_at
             FROM approval_decisions
             WHERE release_intent_id = $1 AND target_environment = $2
             ORDER BY created_at"#,
            intent_id,
            config.target_environment,
        )
        .fetch_all(&self.db)
        .await
        .context("list approval decisions")?;

        let passed = approved_count >= config.required_approvals as i64;
        let reason = if passed {
            format!("approval satisfied: {}/{}", approved_count, config.required_approvals)
        } else {
            format!("awaiting approval: {}/{}", approved_count, config.required_approvals)
        };

        Ok(PolicyEvaluation {
            policy_name: policy_name.to_string(),
            policy_type: PolicyType::Approval,
            passed,
            reason,
            approval_state: Some(ApprovalStateInfo {
                required_approvals: config.required_approvals,
                current_approvals: approved_count as i32,
                decisions: decisions.iter().map(|d| ApprovalDecisionInfo {
                    user_id: d.user_id.to_string(),
                    username: d.username.clone(),
                    decision: d.decision.clone(),
                    decided_at: d.created_at.to_rfc3339(),
                    comment: d.comment.clone(),
                }).collect(),
            }),
        })
    }

    pub async fn record_approval_decision(
        &self,
        release_intent_id: &Uuid,
        policy_id: &Uuid,
        target_environment: &str,
        user_id: &Uuid,
        username: &str,
        decision: &str,
        comment: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query!(
            r#"INSERT INTO approval_decisions (release_intent_id, policy_id, target_environment, user_id, username, decision, comment)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (release_intent_id, target_environment, user_id)
             DO UPDATE SET decision = EXCLUDED.decision, comment = EXCLUDED.comment, created_at = now()"#,
            release_intent_id,
            policy_id,
            target_environment,
            user_id,
            username,
            decision,
            comment,
        )
        .execute(&self.db)
        .await
        .context("record approval decision")?;
        Ok(())
    }

    pub async fn get_intent_actor_id(&self, release_intent_id: &Uuid) -> anyhow::Result<Option<Uuid>> {
        let row = sqlx::query_scalar!(
            "SELECT actor_id FROM release_intents WHERE id = $1",
            release_intent_id,
        )
        .fetch_optional(&self.db)
        .await
        .context("get intent actor")?;
        Ok(row.flatten())
    }

    pub async fn find_approval_policy_for_environment(
        &self,
        project_id: &Uuid,
        target_environment: &str,
    ) -> anyhow::Result<Option<PolicyRecord>> {
        let rec = sqlx::query_as!(
            PolicyRecord,
            r#"SELECT id, project_id, name, enabled, policy_type, config, created_at, updated_at
            FROM policies
            WHERE project_id = $1 AND enabled = true AND policy_type = 'approval'
              AND config->>'target_environment' = $2
            LIMIT 1"#,
            project_id,
            target_environment,
        )
        .fetch_optional(&self.db)
        .await
        .context("find approval policy")?;
        Ok(rec)
    }

    pub async fn get_approval_state_info(
        &self,
        release_intent_id: &Uuid,
        target_environment: &str,
        required_approvals: i32,
    ) -> anyhow::Result<ApprovalStateInfo> {
        let rows = sqlx::query!(
            r#"SELECT user_id, username, decision, comment, created_at
             FROM approval_decisions
             WHERE release_intent_id = $1 AND target_environment = $2
             ORDER BY created_at"#,
            release_intent_id,
            target_environment,
        )
        .fetch_all(&self.db)
        .await
        .context("get approval decisions")?;

        let current_approvals = rows.iter().filter(|r| r.decision == "approved").count() as i32;
        Ok(ApprovalStateInfo {
            required_approvals,
            current_approvals,
            decisions: rows.into_iter().map(|r| ApprovalDecisionInfo {
                user_id: r.user_id.to_string(),
                username: r.username,
                decision: r.decision,
                decided_at: r.created_at.to_rfc3339(),
                comment: r.comment,
            }).collect(),
        })
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
