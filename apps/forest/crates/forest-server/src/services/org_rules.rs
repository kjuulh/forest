use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

use crate::{
    State,
    domains::{
        policy::validate_policy_config,
        trigger::{TriggerPatterns, TriggerTargets},
    },
    services::{
        policy_aggregate::PolicyAggregateServiceState,
        release_pipeline::{
            CreatePipelineParams, PipelineStages, ReleasePipelineRegistryState,
            UpdatePipelineParams, validate_pipeline,
        },
        trigger_aggregate::TriggerAggregateServiceState,
    },
};

const RESOURCE_POLICY: &str = "policy";
const RESOURCE_TRIGGER: &str = "trigger";
const RESOURCE_RELEASE_PIPELINE: &str = "release_pipeline";

#[derive(Clone)]
pub struct OrgRuleSetRegistry {
    db: PgPool,
    state: State,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredProjectSelector {
    #[serde(default)]
    pub include_projects: Vec<String>,
    #[serde(default)]
    pub exclude_projects: Vec<String>,
    #[serde(default)]
    pub name_regex: Option<String>,
    #[serde(default)]
    pub metadata_match: BTreeMap<String, String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredOrgPolicyRule {
    pub name: String,
    pub enabled: bool,
    pub policy_type: String,
    pub config: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoredOrgTriggerRule {
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub branch_pattern: Option<String>,
    #[serde(default)]
    pub title_pattern: Option<String>,
    #[serde(default)]
    pub author_pattern: Option<String>,
    #[serde(default)]
    pub commit_message_pattern: Option<String>,
    #[serde(default)]
    pub source_type_pattern: Option<String>,
    #[serde(default)]
    pub target_environments: Vec<String>,
    #[serde(default)]
    pub target_destinations: Vec<String>,
    pub force_release: bool,
    pub use_pipeline: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredOrgReleasePipelineRule {
    pub name: String,
    pub enabled: bool,
    pub stages: PipelineStages,
}

#[derive(Debug, Clone)]
pub struct OrgRuleSetInput {
    pub organisation: String,
    pub name: String,
    pub enabled: bool,
    pub selector: StoredProjectSelector,
    pub policies: Vec<StoredOrgPolicyRule>,
    pub triggers: Vec<StoredOrgTriggerRule>,
    pub release_pipelines: Vec<StoredOrgReleasePipelineRule>,
}

#[derive(Debug, Clone)]
pub struct OrgRuleSetRecord {
    pub id: Uuid,
    pub organisation: String,
    pub name: String,
    pub enabled: bool,
    pub selector: StoredProjectSelector,
    pub policies: Vec<StoredOrgPolicyRule>,
    pub triggers: Vec<StoredOrgTriggerRule>,
    pub release_pipelines: Vec<StoredOrgReleasePipelineRule>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
struct ProjectSelectionRow {
    id: Uuid,
    project: String,
    metadata: Value,
}

#[derive(Debug)]
struct MaterializationRow {
    id: Uuid,
    project_id: Uuid,
    resource_type: String,
    resource_name: String,
    resource_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DesiredMaterialization {
    project_id: Uuid,
    resource_type: String,
    resource_name: String,
}

impl OrgRuleSetRegistry {
    pub fn new(state: State) -> Self {
        Self {
            db: state.db.clone(),
            state,
        }
    }

    pub async fn create(&self, input: OrgRuleSetInput) -> anyhow::Result<OrgRuleSetRecord> {
        validate_rule_set_input(&input)?;

        let selector = serde_json::to_value(&input.selector).context("serialize selector")?;
        let policies = serde_json::to_value(&input.policies).context("serialize policies")?;
        let triggers = serde_json::to_value(&input.triggers).context("serialize triggers")?;
        let release_pipelines = serde_json::to_value(&input.release_pipelines)
            .context("serialize release pipelines")?;

        let row = sqlx::query(
            r#"INSERT INTO organisation_rule_sets (
                   organisation, name, enabled, selector, policies, triggers, release_pipelines
               ) VALUES ($1, $2, $3, $4, $5, $6, $7)
               RETURNING id, organisation, name, enabled, selector, policies, triggers,
                         release_pipelines, created_at, updated_at"#,
        )
        .bind(&input.organisation)
        .bind(&input.name)
        .bind(input.enabled)
        .bind(selector)
        .bind(policies)
        .bind(triggers)
        .bind(release_pipelines)
        .fetch_one(&self.db)
        .await
        .context("create organisation rule set")?;

        let rec = row_to_rule_set(row)?;
        self.reconcile_rule_set_id(rec.id)
            .await
            .context("reconcile organisation rule set")?;

        self.get_by_id(rec.id)
            .await?
            .context("organisation rule set not found after create")
    }

    pub async fn update(&self, input: OrgRuleSetInput) -> anyhow::Result<OrgRuleSetRecord> {
        validate_rule_set_input(&input)?;

        let selector = serde_json::to_value(&input.selector).context("serialize selector")?;
        let policies = serde_json::to_value(&input.policies).context("serialize policies")?;
        let triggers = serde_json::to_value(&input.triggers).context("serialize triggers")?;
        let release_pipelines = serde_json::to_value(&input.release_pipelines)
            .context("serialize release pipelines")?;

        let row = sqlx::query(
            r#"UPDATE organisation_rule_sets
               SET enabled = $3,
                   selector = $4,
                   policies = $5,
                   triggers = $6,
                   release_pipelines = $7,
                   updated_at = now()
               WHERE organisation = $1 AND name = $2
               RETURNING id, organisation, name, enabled, selector, policies, triggers,
                         release_pipelines, created_at, updated_at"#,
        )
        .bind(&input.organisation)
        .bind(&input.name)
        .bind(input.enabled)
        .bind(selector)
        .bind(policies)
        .bind(triggers)
        .bind(release_pipelines)
        .fetch_optional(&self.db)
        .await
        .context("update organisation rule set")?
        .context("organisation rule set not found")?;

        let rec = row_to_rule_set(row)?;
        self.reconcile_rule_set_id(rec.id)
            .await
            .context("reconcile organisation rule set")?;

        self.get_by_id(rec.id)
            .await?
            .context("organisation rule set not found after update")
    }

    pub async fn delete(&self, organisation: &str, name: &str) -> anyhow::Result<()> {
        let rec = self
            .get_by_org_name(organisation, name)
            .await?
            .context("organisation rule set not found")?;

        self.cleanup_stale(rec.id, None, &BTreeSet::new())
            .await
            .context("delete organisation rule set materializations")?;

        let res = sqlx::query("DELETE FROM organisation_rule_sets WHERE id = $1")
            .bind(rec.id)
            .execute(&self.db)
            .await
            .context("delete organisation rule set")?;

        if res.rows_affected() != 1 {
            bail!("organisation rule set not found");
        }

        Ok(())
    }

    pub async fn list(&self, organisation: &str) -> anyhow::Result<Vec<OrgRuleSetRecord>> {
        let rows = sqlx::query(
            r#"SELECT id, organisation, name, enabled, selector, policies, triggers,
                      release_pipelines, created_at, updated_at
               FROM organisation_rule_sets
               WHERE organisation = $1
               ORDER BY name"#,
        )
        .bind(organisation)
        .fetch_all(&self.db)
        .await
        .context("list organisation rule sets")?;

        rows.into_iter().map(row_to_rule_set).collect()
    }

    pub async fn reconcile_project(&self, organisation: &str, project: &str) -> anyhow::Result<()> {
        let Some(project_row) = self.load_project(organisation, project).await? else {
            return Ok(());
        };

        for rule_set in self.list(organisation).await? {
            self.reconcile_rule_set_for_projects(&rule_set, &[project_row.clone()])
                .await
                .with_context(|| {
                    format!(
                        "reconcile organisation rule set {}/{} for project {}",
                        rule_set.organisation, rule_set.name, project
                    )
                })?;
        }

        Ok(())
    }

    async fn reconcile_rule_set_id(&self, rule_set_id: Uuid) -> anyhow::Result<()> {
        let rule_set = self
            .get_by_id(rule_set_id)
            .await?
            .context("organisation rule set not found")?;
        let projects = self.load_projects(&rule_set.organisation).await?;
        self.reconcile_rule_set_for_projects(&rule_set, &projects)
            .await
    }

    async fn reconcile_rule_set_for_projects(
        &self,
        rule_set: &OrgRuleSetRecord,
        projects: &[ProjectSelectionRow],
    ) -> anyhow::Result<()> {
        let mut desired = BTreeSet::new();
        let project_filter: BTreeSet<Uuid> = projects.iter().map(|p| p.id).collect();

        if rule_set.enabled {
            for project in projects {
                if !selector_matches(&rule_set.selector, project)? {
                    continue;
                }

                for rule in &rule_set.policies {
                    if self
                        .materialize_policy(rule_set.id, project.id, rule)
                        .await?
                    {
                        desired.insert(desired_key(project.id, RESOURCE_POLICY, &rule.name));
                    }
                }

                for rule in &rule_set.triggers {
                    if self
                        .materialize_trigger(rule_set.id, project.id, rule)
                        .await?
                    {
                        desired.insert(desired_key(project.id, RESOURCE_TRIGGER, &rule.name));
                    }
                }

                for rule in &rule_set.release_pipelines {
                    if self
                        .materialize_release_pipeline(rule_set.id, project.id, rule)
                        .await?
                    {
                        desired.insert(desired_key(
                            project.id,
                            RESOURCE_RELEASE_PIPELINE,
                            &rule.name,
                        ));
                    }
                }
            }
        }

        let filter = if project_filter.is_empty() {
            None
        } else {
            Some(&project_filter)
        };
        self.cleanup_stale(rule_set.id, filter, &desired).await
    }

    async fn get_by_id(&self, id: Uuid) -> anyhow::Result<Option<OrgRuleSetRecord>> {
        let row = sqlx::query(
            r#"SELECT id, organisation, name, enabled, selector, policies, triggers,
                      release_pipelines, created_at, updated_at
               FROM organisation_rule_sets
               WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&self.db)
        .await
        .context("get organisation rule set by id")?;

        row.map(row_to_rule_set).transpose()
    }

    async fn get_by_org_name(
        &self,
        organisation: &str,
        name: &str,
    ) -> anyhow::Result<Option<OrgRuleSetRecord>> {
        let row = sqlx::query(
            r#"SELECT id, organisation, name, enabled, selector, policies, triggers,
                      release_pipelines, created_at, updated_at
               FROM organisation_rule_sets
               WHERE organisation = $1 AND name = $2"#,
        )
        .bind(organisation)
        .bind(name)
        .fetch_optional(&self.db)
        .await
        .context("get organisation rule set")?;

        row.map(row_to_rule_set).transpose()
    }

    async fn load_projects(&self, organisation: &str) -> anyhow::Result<Vec<ProjectSelectionRow>> {
        let rows = sqlx::query(
            r#"SELECT id, project, metadata
               FROM projects
               WHERE organisation = $1
               ORDER BY project"#,
        )
        .bind(organisation)
        .fetch_all(&self.db)
        .await
        .context("load projects for organisation rule set")?;

        rows.into_iter().map(row_to_project).collect()
    }

    async fn load_project(
        &self,
        organisation: &str,
        project: &str,
    ) -> anyhow::Result<Option<ProjectSelectionRow>> {
        let row = sqlx::query(
            r#"SELECT id, project, metadata
               FROM projects
               WHERE organisation = $1 AND project = $2"#,
        )
        .bind(organisation)
        .bind(project)
        .fetch_optional(&self.db)
        .await
        .context("load project for organisation rule set")?;

        row.map(row_to_project).transpose()
    }

    async fn materialize_policy(
        &self,
        rule_set_id: Uuid,
        project_id: Uuid,
        rule: &StoredOrgPolicyRule,
    ) -> anyhow::Result<bool> {
        if let Some(materialization) = self
            .find_materialization(rule_set_id, project_id, RESOURCE_POLICY, &rule.name)
            .await?
        {
            if self
                .resource_by_id_exists(RESOURCE_POLICY, materialization.resource_id)
                .await?
            {
                self.state
                    .policy_aggregate_service()
                    .update(
                        &project_id,
                        &rule.name,
                        Some(rule.enabled),
                        Some((rule.policy_type.clone(), rule.config.clone())),
                    )
                    .await
                    .context("update materialized policy")?;
                return Ok(true);
            }

            self.delete_materialization(materialization.id).await?;
        }

        if self
            .resource_exists(RESOURCE_POLICY, project_id, &rule.name)
            .await?
        {
            return Ok(false);
        }

        let rec = self
            .state
            .policy_aggregate_service()
            .create(
                project_id,
                rule.name.clone(),
                rule.policy_type.clone(),
                rule.config.clone(),
            )
            .await
            .context("create materialized policy")?;

        if !rule.enabled {
            self.state
                .policy_aggregate_service()
                .update(&project_id, &rule.name, Some(false), None)
                .await
                .context("disable materialized policy")?;
        }

        self.upsert_materialization(rule_set_id, project_id, RESOURCE_POLICY, &rule.name, rec.id)
            .await?;

        Ok(true)
    }

    async fn materialize_trigger(
        &self,
        rule_set_id: Uuid,
        project_id: Uuid,
        rule: &StoredOrgTriggerRule,
    ) -> anyhow::Result<bool> {
        let patterns = TriggerPatterns {
            branch: rule.branch_pattern.clone(),
            title: rule.title_pattern.clone(),
            author: rule.author_pattern.clone(),
            commit_message: rule.commit_message_pattern.clone(),
            source_type: rule.source_type_pattern.clone(),
        };
        let targets = TriggerTargets {
            environments: rule.target_environments.clone(),
            destinations: rule.target_destinations.clone(),
        };

        if let Some(materialization) = self
            .find_materialization(rule_set_id, project_id, RESOURCE_TRIGGER, &rule.name)
            .await?
        {
            if self
                .resource_by_id_exists(RESOURCE_TRIGGER, materialization.resource_id)
                .await?
            {
                self.state
                    .trigger_aggregate_service()
                    .update(
                        &project_id,
                        &rule.name,
                        Some(rule.enabled),
                        Some(patterns),
                        Some(targets),
                        Some(rule.force_release),
                        Some(rule.use_pipeline),
                    )
                    .await
                    .context("update materialized trigger")?;
                return Ok(true);
            }

            self.delete_materialization(materialization.id).await?;
        }

        if self
            .resource_exists(RESOURCE_TRIGGER, project_id, &rule.name)
            .await?
        {
            return Ok(false);
        }

        let rec = self
            .state
            .trigger_aggregate_service()
            .create(
                project_id,
                rule.name.clone(),
                patterns,
                targets,
                rule.force_release,
                rule.use_pipeline,
            )
            .await
            .context("create materialized trigger")?;

        if !rule.enabled {
            self.state
                .trigger_aggregate_service()
                .update(&project_id, &rule.name, Some(false), None, None, None, None)
                .await
                .context("disable materialized trigger")?;
        }

        self.upsert_materialization(
            rule_set_id,
            project_id,
            RESOURCE_TRIGGER,
            &rule.name,
            rec.id,
        )
        .await?;

        Ok(true)
    }

    async fn materialize_release_pipeline(
        &self,
        rule_set_id: Uuid,
        project_id: Uuid,
        rule: &StoredOrgReleasePipelineRule,
    ) -> anyhow::Result<bool> {
        if let Some(materialization) = self
            .find_materialization(
                rule_set_id,
                project_id,
                RESOURCE_RELEASE_PIPELINE,
                &rule.name,
            )
            .await?
        {
            if self
                .resource_by_id_exists(RESOURCE_RELEASE_PIPELINE, materialization.resource_id)
                .await?
            {
                self.state
                    .release_pipeline_registry()
                    .update(
                        &project_id,
                        &rule.name,
                        UpdatePipelineParams {
                            enabled: Some(rule.enabled),
                            stages: Some(rule.stages.clone()),
                        },
                    )
                    .await
                    .context("update materialized release pipeline")?;
                return Ok(true);
            }

            self.delete_materialization(materialization.id).await?;
        }

        if self
            .resource_exists(RESOURCE_RELEASE_PIPELINE, project_id, &rule.name)
            .await?
        {
            return Ok(false);
        }

        let rec = self
            .state
            .release_pipeline_registry()
            .create(CreatePipelineParams {
                project_id,
                name: rule.name.clone(),
                stages: rule.stages.clone(),
            })
            .await
            .context("create materialized release pipeline")?;

        if !rule.enabled {
            self.state
                .release_pipeline_registry()
                .update(
                    &project_id,
                    &rule.name,
                    UpdatePipelineParams {
                        enabled: Some(false),
                        stages: None,
                    },
                )
                .await
                .context("disable materialized release pipeline")?;
        }

        self.upsert_materialization(
            rule_set_id,
            project_id,
            RESOURCE_RELEASE_PIPELINE,
            &rule.name,
            rec.id,
        )
        .await?;

        Ok(true)
    }

    async fn cleanup_stale(
        &self,
        rule_set_id: Uuid,
        project_filter: Option<&BTreeSet<Uuid>>,
        desired: &BTreeSet<DesiredMaterialization>,
    ) -> anyhow::Result<()> {
        let materializations = if let Some(project_filter) = project_filter {
            let project_ids: Vec<Uuid> = project_filter.iter().copied().collect();
            let rows = sqlx::query(
                r#"SELECT id, project_id, resource_type, resource_name, resource_id
                   FROM organisation_rule_set_materializations
                   WHERE rule_set_id = $1 AND project_id = ANY($2)"#,
            )
            .bind(rule_set_id)
            .bind(project_ids)
            .fetch_all(&self.db)
            .await
            .context("load project materializations")?;
            rows.into_iter()
                .map(row_to_materialization)
                .collect::<anyhow::Result<Vec<_>>>()?
        } else {
            let rows = sqlx::query(
                r#"SELECT id, project_id, resource_type, resource_name, resource_id
                   FROM organisation_rule_set_materializations
                   WHERE rule_set_id = $1"#,
            )
            .bind(rule_set_id)
            .fetch_all(&self.db)
            .await
            .context("load materializations")?;
            rows.into_iter()
                .map(row_to_materialization)
                .collect::<anyhow::Result<Vec<_>>>()?
        };

        for materialization in materializations {
            let key = DesiredMaterialization {
                project_id: materialization.project_id,
                resource_type: materialization.resource_type.clone(),
                resource_name: materialization.resource_name.clone(),
            };

            if desired.contains(&key) {
                continue;
            }

            self.delete_materialized_resource(&materialization).await?;
            self.delete_materialization(materialization.id).await?;
        }

        Ok(())
    }

    async fn find_materialization(
        &self,
        rule_set_id: Uuid,
        project_id: Uuid,
        resource_type: &str,
        resource_name: &str,
    ) -> anyhow::Result<Option<MaterializationRow>> {
        let row = sqlx::query(
            r#"SELECT id, project_id, resource_type, resource_name, resource_id
               FROM organisation_rule_set_materializations
               WHERE rule_set_id = $1
                 AND project_id = $2
                 AND resource_type = $3
                 AND resource_name = $4"#,
        )
        .bind(rule_set_id)
        .bind(project_id)
        .bind(resource_type)
        .bind(resource_name)
        .fetch_optional(&self.db)
        .await
        .context("find organisation rule materialization")?;

        row.map(row_to_materialization).transpose()
    }

    async fn upsert_materialization(
        &self,
        rule_set_id: Uuid,
        project_id: Uuid,
        resource_type: &str,
        resource_name: &str,
        resource_id: Uuid,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"INSERT INTO organisation_rule_set_materializations (
                   rule_set_id, project_id, resource_type, resource_name, resource_id
               ) VALUES ($1, $2, $3, $4, $5)
               ON CONFLICT (rule_set_id, project_id, resource_type, resource_name)
               DO UPDATE SET resource_id = EXCLUDED.resource_id, updated_at = now()"#,
        )
        .bind(rule_set_id)
        .bind(project_id)
        .bind(resource_type)
        .bind(resource_name)
        .bind(resource_id)
        .execute(&self.db)
        .await
        .context("upsert organisation rule materialization")?;

        Ok(())
    }

    async fn delete_materialization(&self, id: Uuid) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM organisation_rule_set_materializations WHERE id = $1")
            .bind(id)
            .execute(&self.db)
            .await
            .context("delete organisation rule materialization")?;
        Ok(())
    }

    async fn delete_materialized_resource(
        &self,
        materialization: &MaterializationRow,
    ) -> anyhow::Result<()> {
        match materialization.resource_type.as_str() {
            RESOURCE_POLICY => {
                self.state
                    .policy_aggregate_service()
                    .delete(&materialization.project_id, &materialization.resource_name)
                    .await
            }
            RESOURCE_TRIGGER => {
                self.state
                    .trigger_aggregate_service()
                    .delete(&materialization.project_id, &materialization.resource_name)
                    .await
            }
            RESOURCE_RELEASE_PIPELINE => {
                self.state
                    .release_pipeline_registry()
                    .delete(&materialization.project_id, &materialization.resource_name)
                    .await
            }
            other => bail!("unknown materialized resource type: {other}"),
        }
        .with_context(|| {
            format!(
                "delete materialized {} {}",
                materialization.resource_type, materialization.resource_name
            )
        })?;

        Ok(())
    }

    async fn resource_exists(
        &self,
        resource_type: &str,
        project_id: Uuid,
        resource_name: &str,
    ) -> anyhow::Result<bool> {
        let query = match resource_type {
            RESOURCE_POLICY => "SELECT id FROM policies WHERE project_id = $1 AND name = $2",
            RESOURCE_TRIGGER => "SELECT id FROM triggers WHERE project_id = $1 AND name = $2",
            RESOURCE_RELEASE_PIPELINE => {
                "SELECT id FROM release_pipelines WHERE project_id = $1 AND name = $2"
            }
            other => bail!("unknown resource type: {other}"),
        };

        let row = sqlx::query(query)
            .bind(project_id)
            .bind(resource_name)
            .fetch_optional(&self.db)
            .await
            .with_context(|| format!("check existing {resource_type} override"))?;

        Ok(row.is_some())
    }

    async fn resource_by_id_exists(
        &self,
        resource_type: &str,
        resource_id: Uuid,
    ) -> anyhow::Result<bool> {
        let query = match resource_type {
            RESOURCE_POLICY => "SELECT id FROM policies WHERE id = $1",
            RESOURCE_TRIGGER => "SELECT id FROM triggers WHERE id = $1",
            RESOURCE_RELEASE_PIPELINE => "SELECT id FROM release_pipelines WHERE id = $1",
            other => bail!("unknown resource type: {other}"),
        };

        let row = sqlx::query(query)
            .bind(resource_id)
            .fetch_optional(&self.db)
            .await
            .with_context(|| format!("check materialized {resource_type} by id"))?;

        Ok(row.is_some())
    }
}

fn row_to_rule_set(row: PgRow) -> anyhow::Result<OrgRuleSetRecord> {
    let selector: Value = row.try_get("selector").context("read selector")?;
    let policies: Value = row.try_get("policies").context("read policies")?;
    let triggers: Value = row.try_get("triggers").context("read triggers")?;
    let release_pipelines: Value = row
        .try_get("release_pipelines")
        .context("read release pipelines")?;

    Ok(OrgRuleSetRecord {
        id: row.try_get("id").context("read id")?,
        organisation: row.try_get("organisation").context("read organisation")?,
        name: row.try_get("name").context("read name")?,
        enabled: row.try_get("enabled").context("read enabled")?,
        selector: serde_json::from_value(selector).context("parse selector")?,
        policies: serde_json::from_value(policies).context("parse policies")?,
        triggers: serde_json::from_value(triggers).context("parse triggers")?,
        release_pipelines: serde_json::from_value(release_pipelines)
            .context("parse release pipelines")?,
        created_at: row.try_get("created_at").context("read created_at")?,
        updated_at: row.try_get("updated_at").context("read updated_at")?,
    })
}

fn row_to_project(row: PgRow) -> anyhow::Result<ProjectSelectionRow> {
    Ok(ProjectSelectionRow {
        id: row.try_get("id").context("read project id")?,
        project: row.try_get("project").context("read project name")?,
        metadata: row.try_get("metadata").context("read project metadata")?,
    })
}

fn row_to_materialization(row: PgRow) -> anyhow::Result<MaterializationRow> {
    Ok(MaterializationRow {
        id: row.try_get("id").context("read materialization id")?,
        project_id: row
            .try_get("project_id")
            .context("read materialization project_id")?,
        resource_type: row
            .try_get("resource_type")
            .context("read materialization resource_type")?,
        resource_name: row
            .try_get("resource_name")
            .context("read materialization resource_name")?,
        resource_id: row
            .try_get("resource_id")
            .context("read materialization resource_id")?,
    })
}

fn desired_key(
    project_id: Uuid,
    resource_type: &str,
    resource_name: &str,
) -> DesiredMaterialization {
    DesiredMaterialization {
        project_id,
        resource_type: resource_type.to_string(),
        resource_name: resource_name.to_string(),
    }
}

fn validate_rule_set_input(input: &OrgRuleSetInput) -> anyhow::Result<()> {
    require_name("organisation", &input.organisation)?;
    require_name("rule set name", &input.name)?;
    validate_selector(&input.selector)?;

    validate_unique_names("policy", input.policies.iter().map(|r| r.name.as_str()))?;
    for rule in &input.policies {
        require_name("policy name", &rule.name)?;
        validate_policy_config(&rule.policy_type, &rule.config)
            .with_context(|| format!("invalid policy rule '{}'", rule.name))?;
    }

    validate_unique_names("trigger", input.triggers.iter().map(|r| r.name.as_str()))?;
    for rule in &input.triggers {
        require_name("trigger name", &rule.name)?;
        validate_trigger_rule(rule)
            .with_context(|| format!("invalid trigger rule '{}'", rule.name))?;
    }

    validate_unique_names(
        "release pipeline",
        input.release_pipelines.iter().map(|r| r.name.as_str()),
    )?;
    for rule in &input.release_pipelines {
        require_name("release pipeline name", &rule.name)?;
        validate_pipeline(&rule.stages)
            .with_context(|| format!("invalid release pipeline rule '{}'", rule.name))?;
    }

    Ok(())
}

fn validate_selector(selector: &StoredProjectSelector) -> anyhow::Result<()> {
    if let Some(pattern) = &selector.name_regex {
        Regex::new(pattern).context("invalid selector name_regex")?;
    }

    for name in selector
        .include_projects
        .iter()
        .chain(selector.exclude_projects.iter())
    {
        require_name("selector project name", name)?;
    }

    for key in selector.metadata_match.keys() {
        require_name("selector metadata key", key)?;
    }

    for tag in &selector.tags {
        require_name("selector tag", tag)?;
    }

    Ok(())
}

fn validate_trigger_rule(rule: &StoredOrgTriggerRule) -> anyhow::Result<()> {
    validate_optional_regex(&rule.branch_pattern, "branch_pattern")?;
    validate_optional_regex(&rule.title_pattern, "title_pattern")?;
    validate_optional_regex(&rule.author_pattern, "author_pattern")?;
    validate_optional_regex(&rule.commit_message_pattern, "commit_message_pattern")?;
    validate_optional_regex(&rule.source_type_pattern, "source_type_pattern")?;

    if !rule.use_pipeline
        && rule.target_environments.is_empty()
        && rule.target_destinations.is_empty()
    {
        bail!(
            "at least one target_environment or target_destination is required (or use_pipeline=true)"
        );
    }

    Ok(())
}

fn validate_optional_regex(pattern: &Option<String>, field: &str) -> anyhow::Result<()> {
    if let Some(pattern) = pattern {
        Regex::new(pattern).with_context(|| format!("invalid regex for {field}"))?;
    }
    Ok(())
}

fn validate_unique_names<'a>(
    resource_type: &str,
    names: impl Iterator<Item = &'a str>,
) -> anyhow::Result<()> {
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            bail!("duplicate {resource_type} rule name '{name}'");
        }
    }
    Ok(())
}

fn require_name(field: &str, value: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        bail!("{field} is required");
    }
    Ok(())
}

fn selector_matches(
    selector: &StoredProjectSelector,
    project: &ProjectSelectionRow,
) -> anyhow::Result<bool> {
    if !selector.include_projects.is_empty()
        && !selector
            .include_projects
            .iter()
            .any(|p| p == &project.project)
    {
        return Ok(false);
    }

    if selector
        .exclude_projects
        .iter()
        .any(|p| p == &project.project)
    {
        return Ok(false);
    }

    if let Some(pattern) = &selector.name_regex {
        let re = Regex::new(pattern).context("invalid selector name_regex")?;
        if !re.is_match(&project.project) {
            return Ok(false);
        }
    }

    for (key, expected) in &selector.metadata_match {
        let Some(actual) = metadata_string(&project.metadata, key) else {
            return Ok(false);
        };
        if actual != expected {
            return Ok(false);
        }
    }
    if !selector.tags.is_empty() {
        let tags = metadata_tags(&project.metadata);
        if !selector.tags.iter().all(|tag| tags.contains(tag.as_str())) {
            return Ok(false);
        }
    }

    Ok(true)
}

fn metadata_string<'a>(metadata: &'a Value, key: &str) -> Option<&'a str> {
    metadata_value_at(metadata, key).and_then(Value::as_str)
}

fn metadata_value_at<'a>(metadata: &'a Value, key: &str) -> Option<&'a Value> {
    let mut current = metadata;
    for part in key.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

fn metadata_tags(metadata: &Value) -> BTreeSet<&str> {
    match metadata.get("tags") {
        Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).collect(),
        Some(Value::String(tags)) => tags
            .split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .collect(),
        _ => BTreeSet::new(),
    }
}

pub trait OrgRuleSetRegistryState {
    fn org_rule_set_registry(&self) -> OrgRuleSetRegistry;
}

impl OrgRuleSetRegistryState for State {
    fn org_rule_set_registry(&self) -> OrgRuleSetRegistry {
        OrgRuleSetRegistry::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, metadata: Value) -> ProjectSelectionRow {
        ProjectSelectionRow {
            id: Uuid::nil(),
            project: name.to_string(),
            metadata,
        }
    }

    #[test]
    fn selector_matches_project_name_metadata_and_tags() {
        let selector = StoredProjectSelector {
            name_regex: Some("^api-".into()),
            metadata_match: BTreeMap::from([("domain".into(), "payments".into())]),
            tags: vec!["backend".into(), "pci".into()],
            ..Default::default()
        };

        assert!(
            selector_matches(
                &selector,
                &project(
                    "api-orders",
                    serde_json::json!({
                        "domain": "payments",
                        "tags": ["backend", "pci", "critical"]
                    })
                ),
            )
            .unwrap()
        );
    }

    #[test]
    fn selector_rejects_excluded_project_even_when_other_terms_match() {
        let selector = StoredProjectSelector {
            include_projects: vec!["api-orders".into()],
            exclude_projects: vec!["api-orders".into()],
            ..Default::default()
        };

        assert!(
            !selector_matches(&selector, &project("api-orders", serde_json::json!({}))).unwrap()
        );
    }

    #[test]
    fn tag_selector_accepts_comma_separated_metadata() {
        let selector = StoredProjectSelector {
            tags: vec!["backend".into(), "pci".into()],
            ..Default::default()
        };

        assert!(
            selector_matches(
                &selector,
                &project("api-orders", serde_json::json!({"tags": "backend, pci"})),
            )
            .unwrap()
        );
    }
}
