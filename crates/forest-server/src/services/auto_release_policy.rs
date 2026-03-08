use anyhow::Context;
use regex::Regex;
use sqlx::PgPool;
use uuid::Uuid;

use crate::State;

use super::release_registry::{ArtifactContext, Reference, Source};

#[derive(Clone)]
pub struct AutoReleasePolicyRegistry {
    db: PgPool,
}

pub struct PolicyRecord {
    pub id: Uuid,
    pub project_id: Uuid,
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
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct CreatePolicyParams {
    pub project_id: Uuid,
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

pub struct UpdatePolicyParams {
    pub enabled: Option<bool>,
    pub branch_pattern: Option<String>,
    pub title_pattern: Option<String>,
    pub author_pattern: Option<String>,
    pub commit_message_pattern: Option<String>,
    pub source_type_pattern: Option<String>,
    pub target_environments: Option<Vec<String>>,
    pub target_destinations: Option<Vec<String>>,
    pub force_release: Option<bool>,
    pub use_pipeline: Option<bool>,
}

/// Data extracted from an annotation, used to evaluate policies.
pub struct AnnotationMatchData {
    pub branch: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub commit_message: Option<String>,
    pub source_type: Option<String>,
}

impl AnnotationMatchData {
    pub fn from_parts(source: &Source, context: &ArtifactContext, reference: &Reference) -> Self {
        Self {
            branch: reference.commit_branch.clone(),
            title: context.title.clone(),
            author: source.username.clone(),
            commit_message: reference.commit_message.clone(),
            source_type: source.source_type.clone(),
        }
    }
}

/// Result of evaluating policies — which policies matched and what to release to.
pub struct PolicyMatch {
    pub policy_name: String,
    pub target_environments: Vec<String>,
    pub target_destinations: Vec<String>,
    pub force_release: bool,
    pub use_pipeline: bool,
}

impl AutoReleasePolicyRegistry {
    pub async fn create(&self, params: CreatePolicyParams) -> anyhow::Result<PolicyRecord> {
        // Validate all regex patterns before inserting
        validate_optional_regex(&params.branch_pattern, "branch_pattern")?;
        validate_optional_regex(&params.title_pattern, "title_pattern")?;
        validate_optional_regex(&params.author_pattern, "author_pattern")?;
        validate_optional_regex(&params.commit_message_pattern, "commit_message_pattern")?;
        validate_optional_regex(&params.source_type_pattern, "source_type_pattern")?;

        if !params.use_pipeline
            && params.target_environments.is_empty()
            && params.target_destinations.is_empty()
        {
            anyhow::bail!("at least one target_environment or target_destination is required (or use_pipeline=true)");
        }

        let rec = sqlx::query_as!(
            PolicyRecord,
            r#"INSERT INTO auto_release_policies (
                project_id, name,
                branch_pattern, title_pattern, author_pattern,
                commit_message_pattern, source_type_pattern,
                target_environments, target_destinations,
                force_release, use_pipeline
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING
                id, project_id, name, enabled,
                branch_pattern, title_pattern, author_pattern,
                commit_message_pattern, source_type_pattern,
                target_environments, target_destinations,
                force_release, use_pipeline, created_at, updated_at"#,
            params.project_id,
            params.name,
            params.branch_pattern,
            params.title_pattern,
            params.author_pattern,
            params.commit_message_pattern,
            params.source_type_pattern,
            &params.target_environments,
            &params.target_destinations,
            params.force_release,
            params.use_pipeline,
        )
        .fetch_one(&self.db)
        .await
        .context("create auto release policy")?;

        Ok(rec)
    }

    pub async fn update(
        &self,
        project_id: &Uuid,
        name: &str,
        params: UpdatePolicyParams,
    ) -> anyhow::Result<PolicyRecord> {
        // Validate regex patterns if provided
        validate_optional_regex(&params.branch_pattern, "branch_pattern")?;
        validate_optional_regex(&params.title_pattern, "title_pattern")?;
        validate_optional_regex(&params.author_pattern, "author_pattern")?;
        validate_optional_regex(&params.commit_message_pattern, "commit_message_pattern")?;
        validate_optional_regex(&params.source_type_pattern, "source_type_pattern")?;

        let rec = sqlx::query_as!(
            PolicyRecord,
            r#"UPDATE auto_release_policies SET
                enabled = COALESCE($3, enabled),
                branch_pattern = COALESCE($4, branch_pattern),
                title_pattern = COALESCE($5, title_pattern),
                author_pattern = COALESCE($6, author_pattern),
                commit_message_pattern = COALESCE($7, commit_message_pattern),
                source_type_pattern = COALESCE($8, source_type_pattern),
                target_environments = COALESCE($9, target_environments),
                target_destinations = COALESCE($10, target_destinations),
                force_release = COALESCE($11, force_release),
                use_pipeline = COALESCE($12, use_pipeline),
                updated_at = now()
            WHERE project_id = $1 AND name = $2
            RETURNING
                id, project_id, name, enabled,
                branch_pattern, title_pattern, author_pattern,
                commit_message_pattern, source_type_pattern,
                target_environments, target_destinations,
                force_release, use_pipeline, created_at, updated_at"#,
            project_id,
            name,
            params.enabled,
            params.branch_pattern,
            params.title_pattern,
            params.author_pattern,
            params.commit_message_pattern,
            params.source_type_pattern,
            params.target_environments.as_deref(),
            params.target_destinations.as_deref(),
            params.force_release,
            params.use_pipeline,
        )
        .fetch_optional(&self.db)
        .await
        .context("update auto release policy")?
        .context("auto release policy not found")?;

        Ok(rec)
    }

    pub async fn delete(&self, project_id: &Uuid, name: &str) -> anyhow::Result<()> {
        let res = sqlx::query!(
            "DELETE FROM auto_release_policies WHERE project_id = $1 AND name = $2",
            project_id,
            name,
        )
        .execute(&self.db)
        .await
        .context("delete auto release policy")?;

        if res.rows_affected() != 1 {
            anyhow::bail!("auto release policy not found");
        }

        Ok(())
    }

    pub async fn list(&self, project_id: &Uuid) -> anyhow::Result<Vec<PolicyRecord>> {
        let recs = sqlx::query_as!(
            PolicyRecord,
            r#"SELECT
                id, project_id, name, enabled,
                branch_pattern, title_pattern, author_pattern,
                commit_message_pattern, source_type_pattern,
                target_environments, target_destinations,
                force_release, use_pipeline, created_at, updated_at
            FROM auto_release_policies
            WHERE project_id = $1
            ORDER BY name"#,
            project_id,
        )
        .fetch_all(&self.db)
        .await
        .context("list auto release policies")?;

        Ok(recs)
    }

    /// Evaluate all enabled policies for a project against the given annotation data.
    /// Returns all matching policies.
    pub async fn evaluate(
        &self,
        project_id: &Uuid,
        data: &AnnotationMatchData,
    ) -> anyhow::Result<Vec<PolicyMatch>> {
        let policies = sqlx::query_as!(
            PolicyRecord,
            r#"SELECT
                id, project_id, name, enabled,
                branch_pattern, title_pattern, author_pattern,
                commit_message_pattern, source_type_pattern,
                target_environments, target_destinations,
                force_release, use_pipeline, created_at, updated_at
            FROM auto_release_policies
            WHERE project_id = $1 AND enabled = true
            ORDER BY name"#,
            project_id,
        )
        .fetch_all(&self.db)
        .await
        .context("evaluate auto release policies")?;

        let mut matches = Vec::new();

        for policy in policies {
            if matches_policy(&policy, data) {
                matches.push(PolicyMatch {
                    policy_name: policy.name,
                    target_environments: policy.target_environments,
                    target_destinations: policy.target_destinations,
                    force_release: policy.force_release,
                    use_pipeline: policy.use_pipeline,
                });
            }
        }

        Ok(matches)
    }
}

fn matches_policy(policy: &PolicyRecord, data: &AnnotationMatchData) -> bool {
    check_pattern(&policy.branch_pattern, data.branch.as_deref())
        && check_pattern(&policy.title_pattern, Some(&data.title))
        && check_pattern(&policy.author_pattern, data.author.as_deref())
        && check_pattern(&policy.commit_message_pattern, data.commit_message.as_deref())
        && check_pattern(&policy.source_type_pattern, data.source_type.as_deref())
}

fn check_pattern(pattern: &Option<String>, value: Option<&str>) -> bool {
    match (pattern, value) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(p), Some(v)) => match Regex::new(p) {
            Ok(re) => re.is_match(v),
            Err(e) => {
                tracing::warn!(pattern = p, "invalid regex in auto-release policy: {e}");
                false
            }
        },
    }
}

fn validate_optional_regex(pattern: &Option<String>, field: &str) -> anyhow::Result<()> {
    if let Some(p) = pattern {
        Regex::new(p).context(format!("invalid regex for {field}"))?;
    }
    Ok(())
}

pub trait AutoReleasePolicyRegistryState {
    fn auto_release_policy_registry(&self) -> AutoReleasePolicyRegistry;
}

impl AutoReleasePolicyRegistryState for State {
    fn auto_release_policy_registry(&self) -> AutoReleasePolicyRegistry {
        AutoReleasePolicyRegistry {
            db: self.db.clone(),
        }
    }
}
