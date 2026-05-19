use anyhow::Context;
use forest_grpc_interface::{
    policy_service_server::PolicyService,
    policy::Config as GrpcPolicyConfig,
    *,
};
use tonic::Response;

use crate::{
    actor::Actor,
    grpc::{artifacts::GrpcErrorExt, authorize},
    services::{
        event_bus::{EventBusState, EventPayload},
        policy::{
            self as policy_svc, PolicyConfig, PolicyRegistryState, PolicyType,
        },
        policy_aggregate::PolicyAggregateServiceState,
        release_registry::ReleaseRegistryState,
        users::UserServiceState,
    },
    state::State,
};

pub struct PoliciesServer {
    pub state: State,
}

fn record_to_grpc(r: policy_svc::PolicyRecord) -> Policy {
    let config = PolicyConfig::from_record(&r.policy_type, &r.config).ok();

    let (policy_type, soak_time, branch_restriction, approval) = match config {
        Some(PolicyConfig::SoakTime(c)) => (
            1, // POLICY_TYPE_SOAK_TIME = 1
            Some(SoakTimeConfig {
                source_environment: c.source_environment,
                target_environment: c.target_environment,
                duration_seconds: c.duration_seconds,
            }),
            None,
            None,
        ),
        Some(PolicyConfig::BranchRestriction(c)) => (
            2, // POLICY_TYPE_BRANCH_RESTRICTION = 2
            None,
            Some(BranchRestrictionConfig {
                target_environment: c.target_environment,
                branch_pattern: c.branch_pattern,
            }),
            None,
        ),
        Some(PolicyConfig::Approval(c)) => (
            3, // POLICY_TYPE_EXTERNAL_APPROVAL = 3
            None,
            None,
            Some(ExternalApprovalConfig {
                target_environment: c.target_environment,
                required_approvals: c.required_approvals,
            }),
        ),
        None => (0, None, None, None),
    };

    Policy {
        id: r.id.to_string(),
        name: r.name,
        enabled: r.enabled,
        policy_type,
        config: match (soak_time, branch_restriction, approval) {
            (Some(st), _, _) => Some(GrpcPolicyConfig::SoakTime(st)),
            (_, Some(br), _) => Some(GrpcPolicyConfig::BranchRestriction(br)),
            (_, _, Some(ac)) => Some(GrpcPolicyConfig::ExternalApproval(ac)),
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
        PolicyType::Approval => 3,
    };
    let external_approval_state = e.approval_state.map(|s| ExternalApprovalState {
        required_approvals: s.required_approvals,
        current_approvals: s.current_approvals,
        decisions: s.decisions.into_iter().map(|d| ExternalApprovalDecisionEntry {
            user_id: d.user_id,
            username: d.username,
            decision: d.decision,
            decided_at: d.decided_at,
            comment: d.comment,
        }).collect(),
    });
    PolicyEvaluation {
        policy_name: e.policy_name,
        policy_type,
        passed: e.passed,
        reason: e.reason,
        external_approval_state,
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
        (3, Some(create_policy_request::Config::ExternalApproval(ac))) => {
            Ok(PolicyConfig::Approval(policy_svc::ApprovalConfig {
                target_environment: ac.target_environment,
                required_approvals: ac.required_approvals,
            }))
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
        Some(update_policy_request::Config::ExternalApproval(ac)) => {
            Ok(Some(PolicyConfig::Approval(policy_svc::ApprovalConfig {
                target_environment: ac.target_environment,
                required_approvals: ac.required_approvals,
            })))
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
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        let _authz = authorize::require_project_access(
            &self.state.db,
            &actor,
            &project,
            authorize::OrgRole::Member,
        )
        .await?;

        let project_id = self
            .state
            .release_registry()
            .get_project_id(&project.organisation, &project.project)
            .await
            .context("resolve project")
            .to_internal_error()?;

        let config = extract_config(req.policy_type, req.config)
            .to_internal_error()?;

        let policy_type = config.policy_type().as_str().to_string();
        let config_json = config.to_json().to_internal_error()?;

        let rec = self
            .state
            .policy_aggregate_service()
            .create(project_id, req.name, policy_type, config_json)
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
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        let _authz = authorize::require_project_access(
            &self.state.db,
            &actor,
            &project,
            authorize::OrgRole::Member,
        )
        .await?;

        let project_id = self
            .state
            .release_registry()
            .get_project_id(&project.organisation, &project.project)
            .await
            .context("resolve project")
            .to_internal_error()?;

        let config = extract_update_config(req.config)
            .to_internal_error()?;

        let config_tuple = match config {
            Some(c) => Some((c.policy_type().as_str().to_string(), c.to_json().to_internal_error()?)),
            None => None,
        };

        let rec = self
            .state
            .policy_aggregate_service()
            .update(&project_id, &req.name, req.enabled, config_tuple)
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
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        let _authz = authorize::require_project_access(
            &self.state.db,
            &actor,
            &project,
            authorize::OrgRole::Member,
        )
        .await?;

        let project_id = self
            .state
            .release_registry()
            .get_project_id(&project.organisation, &project.project)
            .await
            .context("resolve project")
            .to_internal_error()?;

        self.state
            .policy_aggregate_service()
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
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        let _authz = authorize::require_project_access(
            &self.state.db,
            &actor,
            &project,
            authorize::OrgRole::Member,
        )
        .await?;

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
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        let _authz = authorize::require_project_access(
            &self.state.db,
            &actor,
            &project,
            authorize::OrgRole::Member,
        )
        .await?;

        let project_id = self
            .state
            .release_registry()
            .get_project_id(&project.organisation, &project.project)
            .await
            .context("resolve project")
            .to_internal_error()?;

        let release_intent_id = req
            .release_intent_id
            .as_deref()
            .map(|s| s.parse::<uuid::Uuid>())
            .transpose()
            .context("invalid release_intent_id")
            .to_internal_error()?;

        let evaluations = self
            .state
            .policy_registry()
            .evaluate_for_environment(
                &project_id,
                &req.target_environment,
                req.branch.as_deref(),
                release_intent_id.as_ref(),
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

    async fn external_approve_release(
        &self,
        request: tonic::Request<ExternalApproveReleaseRequest>,
    ) -> Result<Response<ExternalApproveReleaseResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;

        let user_id = match &actor {
            Actor::User { user_id } => *user_id,
            _ => return Err(tonic::Status::permission_denied("only users can approve releases")),
        };

        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        let _authz = authorize::require_project_access(
            &self.state.db,
            &actor,
            &project,
            authorize::OrgRole::Member,
        )
        .await?;

        let project_id = self
            .state
            .release_registry()
            .get_project_id(&project.organisation, &project.project)
            .await
            .context("resolve project")
            .to_internal_error()?;

        let release_intent_id: uuid::Uuid = req
            .release_intent_id
            .parse()
            .context("invalid release_intent_id")
            .to_internal_error()?;

        // Prevent self-approval unless force_bypass
        if !req.force_bypass {
            let intent_actor = self
                .state
                .policy_registry()
                .get_intent_actor_id(&release_intent_id)
                .await
                .context("get intent actor")
                .to_internal_error()?;

            if intent_actor == Some(user_id) {
                tracing::warn!(
                    %user_id,
                    %release_intent_id,
                    "approval denied: user is the release author"
                );
                return Err(tonic::Status::permission_denied(
                    "cannot approve your own release intent",
                ));
            }
        }

        let policy = self
            .state
            .policy_registry()
            .find_approval_policy_for_environment(&project_id, &req.target_environment)
            .await
            .context("find approval policy")
            .to_internal_error()?
            .ok_or_else(|| {
                tracing::warn!(
                    target_environment = %req.target_environment,
                    "no approval policy found for environment"
                );
                tonic::Status::not_found("no approval policy found for environment")
            })?;

        let required_approvals = policy
            .config
            .get("required_approvals")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        let user_profile = self
            .state
            .user_service()
            .get_user(user_id)
            .await
            .context("get user profile")
            .to_internal_error()?;

        let username = user_profile
            .map(|u| u.username)
            .unwrap_or_else(|| user_id.to_string());

        self.state
            .policy_registry()
            .record_approval_decision(
                &release_intent_id,
                &policy.id,
                &req.target_environment,
                &user_id,
                &username,
                "approved",
                req.comment.as_deref(),
            )
            .await
            .context("record approval decision")
            .to_internal_error()?;

        // Signal coordinator to re-evaluate
        let _ = self
            .state
            .nats
            .publish(
                "forest.intent.evaluate",
                release_intent_id.to_string().into(),
            )
            .await;

        let state_info = self
            .state
            .policy_registry()
            .get_approval_state_info(&release_intent_id, &req.target_environment, required_approvals)
            .await
            .context("get approval state")
            .to_internal_error()?;

        Ok(Response::new(ExternalApproveReleaseResponse {
            state: Some(ExternalApprovalState {
                required_approvals: state_info.required_approvals,
                current_approvals: state_info.current_approvals,
                decisions: state_info.decisions.into_iter().map(|d| ExternalApprovalDecisionEntry {
                    user_id: d.user_id,
                    username: d.username,
                    decision: d.decision,
                    decided_at: d.decided_at,
                    comment: d.comment,
                }).collect(),
            }),
        }))
    }

    async fn external_reject_release(
        &self,
        request: tonic::Request<ExternalRejectReleaseRequest>,
    ) -> Result<Response<ExternalRejectReleaseResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;

        let user_id = match &actor {
            Actor::User { user_id } => *user_id,
            _ => return Err(tonic::Status::permission_denied("only users can reject releases")),
        };

        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        let _authz = authorize::require_project_access(
            &self.state.db,
            &actor,
            &project,
            authorize::OrgRole::Member,
        )
        .await?;

        let project_id = self
            .state
            .release_registry()
            .get_project_id(&project.organisation, &project.project)
            .await
            .context("resolve project")
            .to_internal_error()?;

        let release_intent_id: uuid::Uuid = req
            .release_intent_id
            .parse()
            .context("invalid release_intent_id")
            .to_internal_error()?;

        let policy = self
            .state
            .policy_registry()
            .find_approval_policy_for_environment(&project_id, &req.target_environment)
            .await
            .context("find approval policy")
            .to_internal_error()?
            .ok_or_else(|| {
                tracing::warn!(
                    target_environment = %req.target_environment,
                    "no approval policy found for environment"
                );
                tonic::Status::not_found("no approval policy found for environment")
            })?;

        let required_approvals = policy
            .config
            .get("required_approvals")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        let user_profile = self
            .state
            .user_service()
            .get_user(user_id)
            .await
            .context("get user profile")
            .to_internal_error()?;

        let username = user_profile
            .map(|u| u.username)
            .unwrap_or_else(|| user_id.to_string());

        self.state
            .policy_registry()
            .record_approval_decision(
                &release_intent_id,
                &policy.id,
                &req.target_environment,
                &user_id,
                &username,
                "rejected",
                req.comment.as_deref(),
            )
            .await
            .context("record rejection decision")
            .to_internal_error()?;

        // Signal coordinator to re-evaluate
        let _ = self
            .state
            .nats
            .publish(
                "forest.intent.evaluate",
                release_intent_id.to_string().into(),
            )
            .await;

        let state_info = self
            .state
            .policy_registry()
            .get_approval_state_info(&release_intent_id, &req.target_environment, required_approvals)
            .await
            .context("get approval state")
            .to_internal_error()?;

        Ok(Response::new(ExternalRejectReleaseResponse {
            state: Some(ExternalApprovalState {
                required_approvals: state_info.required_approvals,
                current_approvals: state_info.current_approvals,
                decisions: state_info.decisions.into_iter().map(|d| ExternalApprovalDecisionEntry {
                    user_id: d.user_id,
                    username: d.username,
                    decision: d.decision,
                    decided_at: d.decided_at,
                    comment: d.comment,
                }).collect(),
            }),
        }))
    }

    async fn get_external_approval_state(
        &self,
        request: tonic::Request<GetExternalApprovalStateRequest>,
    ) -> Result<Response<GetExternalApprovalStateResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        let _authz = authorize::require_project_access(
            &self.state.db,
            &actor,
            &project,
            authorize::OrgRole::Member,
        )
        .await?;

        let project_id = self
            .state
            .release_registry()
            .get_project_id(&project.organisation, &project.project)
            .await
            .context("resolve project")
            .to_internal_error()?;

        let release_intent_id: uuid::Uuid = req
            .release_intent_id
            .parse()
            .context("invalid release_intent_id")
            .to_internal_error()?;

        let policy = self
            .state
            .policy_registry()
            .find_approval_policy_for_environment(&project_id, &req.target_environment)
            .await
            .context("find approval policy")
            .to_internal_error()?
            .ok_or_else(|| {
                tracing::warn!(
                    target_environment = %req.target_environment,
                    "no approval policy found for environment"
                );
                tonic::Status::not_found("no approval policy found for environment")
            })?;

        let required_approvals = policy
            .config
            .get("required_approvals")
            .and_then(|v| v.as_i64())
            .unwrap_or(1) as i32;

        let state_info = self
            .state
            .policy_registry()
            .get_approval_state_info(&release_intent_id, &req.target_environment, required_approvals)
            .await
            .context("get approval state")
            .to_internal_error()?;

        Ok(Response::new(GetExternalApprovalStateResponse {
            state: Some(ExternalApprovalState {
                required_approvals: state_info.required_approvals,
                current_approvals: state_info.current_approvals,
                decisions: state_info.decisions.into_iter().map(|d| ExternalApprovalDecisionEntry {
                    user_id: d.user_id,
                    username: d.username,
                    decision: d.decision,
                    decided_at: d.decided_at,
                    comment: d.comment,
                }).collect(),
            }),
        }))
    }
}
