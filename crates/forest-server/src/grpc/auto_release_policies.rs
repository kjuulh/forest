use anyhow::Context;
use forest_grpc_interface::{auto_release_policy_service_server::AutoReleasePolicyService, *};
use tonic::Response;

use crate::{
    grpc::artifacts::GrpcErrorExt,
    services::{
        auto_release_policy::{
            AutoReleasePolicyRegistryState, CreatePolicyParams, UpdatePolicyParams,
        },
        release_registry::ReleaseRegistryState,
    },
    state::State,
};

pub struct AutoReleasePoliciesServer {
    pub state: State,
}

fn record_to_grpc(
    r: crate::services::auto_release_policy::PolicyRecord,
) -> AutoReleasePolicy {
    AutoReleasePolicy {
        id: r.id.to_string(),
        name: r.name,
        enabled: r.enabled,
        branch_pattern: r.branch_pattern,
        title_pattern: r.title_pattern,
        author_pattern: r.author_pattern,
        commit_message_pattern: r.commit_message_pattern,
        source_type_pattern: r.source_type_pattern,
        target_environments: r.target_environments,
        target_destinations: r.target_destinations,
        force_release: r.force_release,
        use_pipeline: r.use_pipeline,
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
    }
}

#[async_trait::async_trait]
impl AutoReleasePolicyService for AutoReleasePoliciesServer {
    async fn create_auto_release_policy(
        &self,
        request: tonic::Request<CreateAutoReleasePolicyRequest>,
    ) -> Result<Response<CreateAutoReleasePolicyResponse>, tonic::Status> {
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

        let rec = self
            .state
            .auto_release_policy_registry()
            .create(CreatePolicyParams {
                project_id,
                name: req.name,
                branch_pattern: req.branch_pattern,
                title_pattern: req.title_pattern,
                author_pattern: req.author_pattern,
                commit_message_pattern: req.commit_message_pattern,
                source_type_pattern: req.source_type_pattern,
                target_environments: req.target_environments,
                target_destinations: req.target_destinations,
                force_release: req.force_release,
                use_pipeline: req.use_pipeline,
            })
            .await
            .context("create auto release policy")
            .to_internal_error()?;

        Ok(Response::new(CreateAutoReleasePolicyResponse {
            policy: Some(record_to_grpc(rec)),
        }))
    }

    async fn update_auto_release_policy(
        &self,
        request: tonic::Request<UpdateAutoReleasePolicyRequest>,
    ) -> Result<Response<UpdateAutoReleasePolicyResponse>, tonic::Status> {
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

        let rec = self
            .state
            .auto_release_policy_registry()
            .update(
                &project_id,
                &req.name,
                UpdatePolicyParams {
                    enabled: req.enabled,
                    branch_pattern: req.branch_pattern,
                    title_pattern: req.title_pattern,
                    author_pattern: req.author_pattern,
                    commit_message_pattern: req.commit_message_pattern,
                    source_type_pattern: req.source_type_pattern,
                    target_environments: if req.target_environments.is_empty() {
                        None
                    } else {
                        Some(req.target_environments)
                    },
                    target_destinations: if req.target_destinations.is_empty() {
                        None
                    } else {
                        Some(req.target_destinations)
                    },
                    force_release: req.force_release,
                    use_pipeline: req.use_pipeline,
                },
            )
            .await
            .context("update auto release policy")
            .to_internal_error()?;

        Ok(Response::new(UpdateAutoReleasePolicyResponse {
            policy: Some(record_to_grpc(rec)),
        }))
    }

    async fn delete_auto_release_policy(
        &self,
        request: tonic::Request<DeleteAutoReleasePolicyRequest>,
    ) -> Result<Response<DeleteAutoReleasePolicyResponse>, tonic::Status> {
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
            .auto_release_policy_registry()
            .delete(&project_id, &req.name)
            .await
            .context("delete auto release policy")
            .to_internal_error()?;

        Ok(Response::new(DeleteAutoReleasePolicyResponse {}))
    }

    async fn list_auto_release_policies(
        &self,
        request: tonic::Request<ListAutoReleasePoliciesRequest>,
    ) -> Result<Response<ListAutoReleasePoliciesResponse>, tonic::Status> {
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
            .auto_release_policy_registry()
            .list(&project_id)
            .await
            .context("list auto release policies")
            .to_internal_error()?;

        Ok(Response::new(ListAutoReleasePoliciesResponse {
            policies: recs.into_iter().map(record_to_grpc).collect(),
        }))
    }
}
