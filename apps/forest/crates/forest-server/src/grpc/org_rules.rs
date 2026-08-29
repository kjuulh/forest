use anyhow::Context;
use forest_grpc_interface::{
    org_policy_rule, org_rule_set_service_server::OrgRuleSetService, pipeline_stage, *,
};
use tonic::Response;

use crate::{
    grpc::{artifacts::GrpcErrorExt, authorize},
    services::{
        event_bus::{EventBusState, EventPayload},
        org_rules::{
            OrgRuleSetInput, OrgRuleSetRecord, OrgRuleSetRegistryState,
            StoredOrgPolicyRule as SvcOrgPolicyRule,
            StoredOrgReleasePipelineRule as SvcOrgReleasePipelineRule,
            StoredOrgTriggerRule as SvcOrgTriggerRule, StoredProjectSelector as SvcProjectSelector,
        },
        release_pipeline::{PipelineStages, StageConfig, StageDefinition},
    },
    state::State,
};

pub struct OrgRulesServer {
    pub state: State,
}

fn selector_from_proto(selector: Option<ProjectSelector>) -> SvcProjectSelector {
    let selector = selector.unwrap_or_default();
    SvcProjectSelector {
        include_projects: selector.include_projects,
        exclude_projects: selector.exclude_projects,
        name_regex: selector.name_regex,
        metadata_match: selector.metadata_match.into_iter().collect(),
        tags: selector.tags,
    }
}

fn selector_to_proto(selector: SvcProjectSelector) -> ProjectSelector {
    ProjectSelector {
        include_projects: selector.include_projects,
        exclude_projects: selector.exclude_projects,
        name_regex: selector.name_regex,
        metadata_match: selector.metadata_match.into_iter().collect(),
        tags: selector.tags,
    }
}

fn policy_from_proto(rule: OrgPolicyRule) -> anyhow::Result<SvcOrgPolicyRule> {
    let (policy_type, config) = match (rule.policy_type, rule.config) {
        (1, Some(org_policy_rule::Config::SoakTime(c))) => (
            "soak_time".to_string(),
            serde_json::to_value(crate::services::policy::SoakTimeConfig {
                source_environment: c.source_environment,
                target_environment: c.target_environment,
                duration_seconds: c.duration_seconds,
            })?,
        ),
        (2, Some(org_policy_rule::Config::BranchRestriction(c))) => (
            "branch_restriction".to_string(),
            serde_json::to_value(crate::services::policy::BranchRestrictionConfig {
                target_environment: c.target_environment,
                branch_pattern: c.branch_pattern,
            })?,
        ),
        (3, Some(org_policy_rule::Config::ExternalApproval(c))) => (
            "approval".to_string(),
            serde_json::to_value(crate::services::policy::ApprovalConfig {
                target_environment: c.target_environment,
                required_approvals: c.required_approvals,
            })?,
        ),
        (_, None) => anyhow::bail!("org policy rule '{}' is missing config", rule.name),
        _ => anyhow::bail!(
            "org policy rule '{}' policy_type and config do not match",
            rule.name
        ),
    };
    Ok(SvcOrgPolicyRule {
        name: rule.name,
        enabled: rule.enabled,
        policy_type,
        config,
    })
}

fn policy_to_proto(rule: SvcOrgPolicyRule) -> OrgPolicyRule {
    let config = match rule.policy_type.as_str() {
        "soak_time" => {
            serde_json::from_value::<crate::services::policy::SoakTimeConfig>(rule.config)
                .ok()
                .map(|c| {
                    org_policy_rule::Config::SoakTime(SoakTimeConfig {
                        source_environment: c.source_environment,
                        target_environment: c.target_environment,
                        duration_seconds: c.duration_seconds,
                    })
                })
        }
        "branch_restriction" => {
            serde_json::from_value::<crate::services::policy::BranchRestrictionConfig>(rule.config)
                .ok()
                .map(|c| {
                    org_policy_rule::Config::BranchRestriction(BranchRestrictionConfig {
                        target_environment: c.target_environment,
                        branch_pattern: c.branch_pattern,
                    })
                })
        }
        "approval" => {
            serde_json::from_value::<crate::services::policy::ApprovalConfig>(rule.config)
                .ok()
                .map(|c| {
                    org_policy_rule::Config::ExternalApproval(ExternalApprovalConfig {
                        target_environment: c.target_environment,
                        required_approvals: c.required_approvals,
                    })
                })
        }
        _ => None,
    };
    let policy_type = match rule.policy_type.as_str() {
        "soak_time" => 1,
        "branch_restriction" => 2,
        "approval" => 3,
        _ => 0,
    };
    OrgPolicyRule {
        name: rule.name,
        enabled: rule.enabled,
        policy_type,
        config,
    }
}

fn trigger_from_proto(rule: OrgTriggerRule) -> SvcOrgTriggerRule {
    SvcOrgTriggerRule {
        name: rule.name,
        enabled: rule.enabled,
        branch_pattern: rule.branch_pattern,
        title_pattern: rule.title_pattern,
        author_pattern: rule.author_pattern,
        commit_message_pattern: rule.commit_message_pattern,
        source_type_pattern: rule.source_type_pattern,
        target_environments: rule.target_environments,
        target_destinations: rule.target_destinations,
        force_release: rule.force_release,
        use_pipeline: rule.use_pipeline,
    }
}

fn trigger_to_proto(rule: SvcOrgTriggerRule) -> OrgTriggerRule {
    OrgTriggerRule {
        name: rule.name,
        enabled: rule.enabled,
        branch_pattern: rule.branch_pattern,
        title_pattern: rule.title_pattern,
        author_pattern: rule.author_pattern,
        commit_message_pattern: rule.commit_message_pattern,
        source_type_pattern: rule.source_type_pattern,
        target_environments: rule.target_environments,
        target_destinations: rule.target_destinations,
        force_release: rule.force_release,
        use_pipeline: rule.use_pipeline,
    }
}

fn stages_from_proto(proto_stages: Vec<PipelineStage>) -> anyhow::Result<PipelineStages> {
    let mut stages = PipelineStages::new();
    for ps in proto_stages {
        if ps.id.is_empty() {
            anyhow::bail!("stage id must not be empty");
        }
        let config = match ps.config {
            Some(pipeline_stage::Config::Deploy(c)) => StageConfig::Deploy {
                environment: c.environment,
            },
            Some(pipeline_stage::Config::Wait(c)) => StageConfig::Wait {
                duration_seconds: c.duration_seconds,
            },
            Some(pipeline_stage::Config::Plan(c)) => StageConfig::Plan {
                environment: c.environment,
                auto_approve: c.auto_approve,
            },
            None => anyhow::bail!("stage '{}' is missing a config", ps.id),
        };
        if stages
            .insert(
                ps.id.clone(),
                StageDefinition {
                    depends_on: ps.depends_on,
                    config,
                },
            )
            .is_some()
        {
            anyhow::bail!("duplicate stage id '{}'", ps.id);
        }
    }
    Ok(stages)
}

fn stages_to_proto(stages: PipelineStages) -> Vec<PipelineStage> {
    stages
        .into_iter()
        .map(|(id, def)| {
            let config = match def.config {
                StageConfig::Deploy { environment } => {
                    Some(pipeline_stage::Config::Deploy(DeployStageConfig {
                        environment,
                    }))
                }
                StageConfig::Wait { duration_seconds } => {
                    Some(pipeline_stage::Config::Wait(WaitStageConfig {
                        duration_seconds,
                    }))
                }
                StageConfig::Plan {
                    environment,
                    auto_approve,
                } => Some(pipeline_stage::Config::Plan(PlanStageConfig {
                    environment,
                    auto_approve,
                })),
            };
            PipelineStage {
                id,
                depends_on: def.depends_on,
                config,
            }
        })
        .collect()
}

fn pipeline_from_proto(rule: OrgReleasePipelineRule) -> anyhow::Result<SvcOrgReleasePipelineRule> {
    Ok(SvcOrgReleasePipelineRule {
        name: rule.name,
        enabled: rule.enabled,
        stages: stages_from_proto(rule.stages)?,
    })
}

fn pipeline_to_proto(rule: SvcOrgReleasePipelineRule) -> OrgReleasePipelineRule {
    OrgReleasePipelineRule {
        name: rule.name,
        enabled: rule.enabled,
        stages: stages_to_proto(rule.stages),
    }
}

fn record_to_proto(record: OrgRuleSetRecord) -> OrgRuleSet {
    OrgRuleSet {
        organisation: record.organisation,
        name: record.name,
        enabled: record.enabled,
        selector: Some(selector_to_proto(record.selector)),
        policies: record.policies.into_iter().map(policy_to_proto).collect(),
        triggers: record.triggers.into_iter().map(trigger_to_proto).collect(),
        release_pipelines: record
            .release_pipelines
            .into_iter()
            .map(pipeline_to_proto)
            .collect(),
        created_at: record.created_at.to_rfc3339(),
        updated_at: record.updated_at.to_rfc3339(),
    }
}

fn split_rule_set(
    rule_set: OrgRuleSet,
) -> anyhow::Result<(
    String,
    String,
    bool,
    SvcProjectSelector,
    Vec<SvcOrgPolicyRule>,
    Vec<SvcOrgTriggerRule>,
    Vec<SvcOrgReleasePipelineRule>,
)> {
    if rule_set.organisation.trim().is_empty() {
        anyhow::bail!("organisation is required");
    }
    if rule_set.name.trim().is_empty() {
        anyhow::bail!("name is required");
    }
    let policies = rule_set
        .policies
        .into_iter()
        .map(policy_from_proto)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let triggers = rule_set
        .triggers
        .into_iter()
        .map(trigger_from_proto)
        .collect();
    let release_pipelines = rule_set
        .release_pipelines
        .into_iter()
        .map(pipeline_from_proto)
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok((
        rule_set.organisation,
        rule_set.name,
        rule_set.enabled,
        selector_from_proto(rule_set.selector),
        policies,
        triggers,
        release_pipelines,
    ))
}

#[async_trait::async_trait]
impl OrgRuleSetService for OrgRulesServer {
    async fn create_org_rule_set(
        &self,
        request: tonic::Request<CreateOrgRuleSetRequest>,
    ) -> Result<Response<CreateOrgRuleSetResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        let rule_set = req
            .rule_set
            .context("rule_set is required")
            .to_internal_error()?;
        let (organisation, name, enabled, selector, policies, triggers, release_pipelines) =
            split_rule_set(rule_set).to_internal_error()?;

        authorize::require_org_access(
            &self.state.db,
            &actor,
            &organisation,
            authorize::OrgRole::Admin,
        )
        .await?;

        let rec = self
            .state
            .org_rule_set_registry()
            .create(OrgRuleSetInput {
                organisation: organisation.clone(),
                name: name.clone(),
                enabled,
                selector,
                policies,
                triggers,
                release_pipelines,
            })
            .await
            .context("create org rule set")
            .to_internal_error()?;

        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: organisation.clone(),
                project: String::new(),
                resource_type: "org_rule_set",
                action: "created",
                resource_id: name,
                metadata: Default::default(),
            })
            .await;

        Ok(Response::new(CreateOrgRuleSetResponse {
            rule_set: Some(record_to_proto(rec)),
        }))
    }

    async fn update_org_rule_set(
        &self,
        request: tonic::Request<UpdateOrgRuleSetRequest>,
    ) -> Result<Response<UpdateOrgRuleSetResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        let rule_set = req
            .rule_set
            .context("rule_set is required")
            .to_internal_error()?;
        let (organisation, name, enabled, selector, policies, triggers, release_pipelines) =
            split_rule_set(rule_set).to_internal_error()?;

        authorize::require_org_access(
            &self.state.db,
            &actor,
            &organisation,
            authorize::OrgRole::Admin,
        )
        .await?;

        let rec = self
            .state
            .org_rule_set_registry()
            .update(OrgRuleSetInput {
                organisation: organisation.clone(),
                name: name.clone(),
                enabled,
                selector,
                policies,
                triggers,
                release_pipelines,
            })
            .await
            .context("update org rule set")
            .to_internal_error()?;

        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: organisation.clone(),
                project: String::new(),
                resource_type: "org_rule_set",
                action: "updated",
                resource_id: name,
                metadata: Default::default(),
            })
            .await;

        Ok(Response::new(UpdateOrgRuleSetResponse {
            rule_set: Some(record_to_proto(rec)),
        }))
    }

    async fn delete_org_rule_set(
        &self,
        request: tonic::Request<DeleteOrgRuleSetRequest>,
    ) -> Result<Response<DeleteOrgRuleSetResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Admin,
        )
        .await?;

        self.state
            .org_rule_set_registry()
            .delete(&req.organisation, &req.name)
            .await
            .context("delete org rule set")
            .to_internal_error()?;

        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: req.organisation,
                project: String::new(),
                resource_type: "org_rule_set",
                action: "deleted",
                resource_id: req.name,
                metadata: Default::default(),
            })
            .await;

        Ok(Response::new(DeleteOrgRuleSetResponse {}))
    }

    async fn list_org_rule_sets(
        &self,
        request: tonic::Request<ListOrgRuleSetsRequest>,
    ) -> Result<Response<ListOrgRuleSetsResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();
        authorize::require_org_access(
            &self.state.db,
            &actor,
            &req.organisation,
            authorize::OrgRole::Member,
        )
        .await?;

        let rule_sets = self
            .state
            .org_rule_set_registry()
            .list(&req.organisation)
            .await
            .context("list org rule sets")
            .to_internal_error()?
            .into_iter()
            .map(record_to_proto)
            .collect();

        Ok(Response::new(ListOrgRuleSetsResponse { rule_sets }))
    }
}
