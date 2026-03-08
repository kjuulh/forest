use anyhow::Context;
use forest_grpc_interface::{
    policy_service_server::PolicyService,
    policy::Config as GrpcPolicyConfig,
    *,
};
use tonic::Response;

use crate::{
    grpc::artifacts::GrpcErrorExt,
    services::{
        event_bus::{EventBusState, EventPayload},
        policy::{
            self as policy_svc, CreatePolicyParams, PolicyConfig, PolicyRegistryState, PolicyType,
            UpdatePolicyParams,
        },
        release_registry::ReleaseRegistryState,
    },
    state::State,
};

pub struct PoliciesServer {
    pub state: State,
}

fn record_to_grpc(r: policy_svc::PolicyRecord) -> Policy {
    let config = PolicyConfig::from_record(&r.policy_type, &r.config).ok();

    let (policy_type, soak_time, branch_restriction) = match config {
        Some(PolicyConfig::SoakTime(c)) => (
            PolicyType::SoakTime as i32 + 1, // POLICY_TYPE_SOAK_TIME = 1
            Some(SoakTimeConfig {
                source_environment: c.source_environment,
                target_environment: c.target_environment,
                duration_seconds: c.duration_seconds,
            }),
            None,
        ),
        Some(PolicyConfig::BranchRestriction(c)) => (
            2, // POLICY_TYPE_BRANCH_RESTRICTION = 2
            None,
            Some(BranchRestrictionConfig {
                target_environment: c.target_environment,
                branch_pattern: c.branch_pattern,
            }),
        ),
        None => (0, None, None),
    };

    Policy {
        id: r.id.to_string(),
        name: r.name,
        enabled: r.enabled,
        policy_type,
        config: match (soak_time, branch_restriction) {
            (Some(st), _) => Some(GrpcPolicyConfig::SoakTime(st)),
            (_, Some(br)) => Some(GrpcPolicyConfig::BranchRestriction(br)),
            _ => None,
        },
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
    }
}

fn eval_to_grpc(e: policy_svc::PolicyEvaluation) -> PolicyEvaluation {
    let policy_type = match e.policy_type {
        PolicyType::SoakTime => 1,
        PolicyType::BranchRestriction => 2,
    };
    PolicyEvaluation {
        policy_name: e.policy_name,
        policy_type,
        passed: e.passed,
        reason: e.reason,
    }
}

fn extract_config(
    policy_type: i32,
    config: Option<create_policy_request::Config>,
) -> anyhow::Result<PolicyConfig> {
    match (policy_type, config) {
        (1, Some(create_policy_request::Config::SoakTime(st))) => {
            Ok(PolicyConfig::SoakTime(policy_svc::SoakTimeConfig {
                source_environment: st.source_environment,
                target_environment: st.target_environment,
                duration_seconds: st.duration_seconds,
            }))
        }
        (2, Some(create_policy_request::Config::BranchRestriction(br))) => {
            Ok(PolicyConfig::BranchRestriction(
                policy_svc::BranchRestrictionConfig {
                    target_environment: br.target_environment,
                    branch_pattern: br.branch_pattern,
                },
            ))
        }
        (_, None) => anyhow::bail!("config is required"),
        _ => anyhow::bail!("policy_type and config must match"),
    }
}

fn extract_update_config(
    config: Option<update_policy_request::Config>,
) -> anyhow::Result<Option<PolicyConfig>> {
    match config {
        Some(update_policy_request::Config::SoakTime(st)) => {
            Ok(Some(PolicyConfig::SoakTime(policy_svc::SoakTimeConfig {
                source_environment: st.source_environment,
                target_environment: st.target_environment,
                duration_seconds: st.duration_seconds,
            })))
        }
        Some(update_policy_request::Config::BranchRestriction(br)) => {
            Ok(Some(PolicyConfig::BranchRestriction(
                policy_svc::BranchRestrictionConfig {
                    target_environment: br.target_environment,
                    branch_pattern: br.branch_pattern,
                },
            )))
        }
        None => Ok(None),
    }
}

#[async_trait::async_trait]
impl PolicyService for PoliciesServer {
    async fn create_policy(
        &self,
        request: tonic::Request<CreatePolicyRequest>,
    ) -> Result<Response<CreatePolicyResponse>, tonic::Status> {
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        let project_id = self
            .state
            .release_registry()
            .get_project_id(&project.organisation, &project.project)
            .await
            .context("resolve project")
            .to_internal_error()?;

        let config = extract_config(req.policy_type, req.config)
            .to_internal_error()?;

        let rec = self
            .state
            .policy_registry()
            .create(CreatePolicyParams {
                project_id,
                name: req.name,
                config,
            })
            .await
            .context("create policy")
            .to_internal_error()?;

        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: project.organisation.clone(),
                project: project.project.clone(),
                resource_type: "policy",
                action: "created",
                resource_id: rec.id.to_string(),
                metadata: [("name".into(), rec.name.clone())].into(),
            })
            .await;

        Ok(Response::new(CreatePolicyResponse {
            policy: Some(record_to_grpc(rec)),
        }))
    }

    async fn update_policy(
        &self,
        request: tonic::Request<UpdatePolicyRequest>,
    ) -> Result<Response<UpdatePolicyResponse>, tonic::Status> {
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        let project_id = self
            .state
            .release_registry()
            .get_project_id(&project.organisation, &project.project)
            .await
            .context("resolve project")
            .to_internal_error()?;

        let config = extract_update_config(req.config)
            .to_internal_error()?;

        let rec = self
            .state
            .policy_registry()
            .update(
                &project_id,
                &req.name,
                UpdatePolicyParams {
                    enabled: req.enabled,
                    config,
                },
            )
            .await
            .context("update policy")
            .to_internal_error()?;

        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: project.organisation.clone(),
                project: project.project.clone(),
                resource_type: "policy",
                action: "updated",
                resource_id: rec.id.to_string(),
                metadata: [("name".into(), rec.name.clone())].into(),
            })
            .await;

        Ok(Response::new(UpdatePolicyResponse {
            policy: Some(record_to_grpc(rec)),
        }))
    }

    async fn delete_policy(
        &self,
        request: tonic::Request<DeletePolicyRequest>,
    ) -> Result<Response<DeletePolicyResponse>, tonic::Status> {
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        let project_id = self
            .state
            .release_registry()
            .get_project_id(&project.organisation, &project.project)
            .await
            .context("resolve project")
            .to_internal_error()?;

        self.state
            .policy_registry()
            .delete(&project_id, &req.name)
            .await
            .context("delete policy")
            .to_internal_error()?;

        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: project.organisation.clone(),
                project: project.project.clone(),
                resource_type: "policy",
                action: "deleted",
                resource_id: req.name.clone(),
                metadata: Default::default(),
            })
            .await;

        Ok(Response::new(DeletePolicyResponse {}))
    }

    async fn list_policies(
        &self,
        request: tonic::Request<ListPoliciesRequest>,
    ) -> Result<Response<ListPoliciesResponse>, tonic::Status> {
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        let project_id = self
            .state
            .release_registry()
            .get_project_id(&project.organisation, &project.project)
            .await
            .context("resolve project")
            .to_internal_error()?;

        let recs = self
            .state
            .policy_registry()
            .list(&project_id)
            .await
            .context("list policies")
            .to_internal_error()?;

        Ok(Response::new(ListPoliciesResponse {
            policies: recs.into_iter().map(record_to_grpc).collect(),
        }))
    }

    async fn evaluate_policies(
        &self,
        request: tonic::Request<EvaluatePoliciesRequest>,
    ) -> Result<Response<EvaluatePoliciesResponse>, tonic::Status> {
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        let project_id = self
            .state
            .release_registry()
            .get_project_id(&project.organisation, &project.project)
            .await
            .context("resolve project")
            .to_internal_error()?;

        let evaluations = self
            .state
            .policy_registry()
            .evaluate_for_environment(
                &project_id,
                &req.target_environment,
                req.branch.as_deref(),
            )
            .await
            .context("evaluate policies")
            .to_internal_error()?;

        let all_passed = evaluations.iter().all(|e| e.passed);

        Ok(Response::new(EvaluatePoliciesResponse {
            evaluations: evaluations.into_iter().map(eval_to_grpc).collect(),
            all_passed,
        }))
    }
}
