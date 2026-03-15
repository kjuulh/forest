use anyhow::Context;
use forest_grpc_interface::{trigger_service_server::TriggerService, *};
use tonic::Response;

use crate::{
    domains::trigger::{TriggerPatterns, TriggerTargets},
    grpc::{artifacts::GrpcErrorExt, authorize},
    services::{
        trigger_aggregate::{TriggerAggregateServiceState, TriggerRecord},
        event_bus::{EventBusState, EventPayload},
        release_registry::ReleaseRegistryState,
    },
    state::State,
};

pub struct TriggersServer {
    pub state: State,
}

fn record_to_grpc(r: TriggerRecord) -> Trigger {
    Trigger {
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
impl TriggerService for TriggersServer {
    async fn create_trigger(
        &self,
        request: tonic::Request<CreateTriggerRequest>,
    ) -> Result<Response<CreateTriggerResponse>, tonic::Status> {
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

        let rec = self
            .state
            .trigger_aggregate_service()
            .create(
                project_id,
                req.name,
                TriggerPatterns {
                    branch: req.branch_pattern,
                    title: req.title_pattern,
                    author: req.author_pattern,
                    commit_message: req.commit_message_pattern,
                    source_type: req.source_type_pattern,
                },
                TriggerTargets {
                    environments: req.target_environments,
                    destinations: req.target_destinations,
                },
                req.force_release,
                req.use_pipeline,
            )
            .await
            .context("create trigger")
            .to_internal_error()?;

        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: project.organisation.clone(),
                project: project.project.clone(),
                resource_type: "trigger",
                action: "created",
                resource_id: rec.id.to_string(),
                metadata: [("name".into(), rec.name.clone())].into(),
            })
            .await;

        Ok(Response::new(CreateTriggerResponse {
            trigger: Some(record_to_grpc(rec)),
        }))
    }

    async fn update_trigger(
        &self,
        request: tonic::Request<UpdateTriggerRequest>,
    ) -> Result<Response<UpdateTriggerResponse>, tonic::Status> {
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

        // Build patterns only if any pattern field is set
        let patterns = if req.branch_pattern.is_some()
            || req.title_pattern.is_some()
            || req.author_pattern.is_some()
            || req.commit_message_pattern.is_some()
            || req.source_type_pattern.is_some()
        {
            Some(TriggerPatterns {
                branch: req.branch_pattern,
                title: req.title_pattern,
                author: req.author_pattern,
                commit_message: req.commit_message_pattern,
                source_type: req.source_type_pattern,
            })
        } else {
            None
        };

        let targets = if !req.target_environments.is_empty() || !req.target_destinations.is_empty()
        {
            Some(TriggerTargets {
                environments: req.target_environments,
                destinations: req.target_destinations,
            })
        } else {
            None
        };

        let rec = self
            .state
            .trigger_aggregate_service()
            .update(
                &project_id,
                &req.name,
                req.enabled,
                patterns,
                targets,
                req.force_release,
                req.use_pipeline,
            )
            .await
            .context("update trigger")
            .to_internal_error()?;

        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: project.organisation.clone(),
                project: project.project.clone(),
                resource_type: "trigger",
                action: "updated",
                resource_id: rec.id.to_string(),
                metadata: [("name".into(), rec.name.clone())].into(),
            })
            .await;

        Ok(Response::new(UpdateTriggerResponse {
            trigger: Some(record_to_grpc(rec)),
        }))
    }

    async fn delete_trigger(
        &self,
        request: tonic::Request<DeleteTriggerRequest>,
    ) -> Result<Response<DeleteTriggerResponse>, tonic::Status> {
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
            .trigger_aggregate_service()
            .delete(&project_id, &req.name)
            .await
            .context("delete trigger")
            .to_internal_error()?;

        self.state
            .event_bus()
            .emit(EventPayload {
                organisation: project.organisation.clone(),
                project: project.project.clone(),
                resource_type: "trigger",
                action: "deleted",
                resource_id: req.name.clone(),
                metadata: Default::default(),
            })
            .await;

        Ok(Response::new(DeleteTriggerResponse {}))
    }

    async fn list_triggers(
        &self,
        request: tonic::Request<ListTriggersRequest>,
    ) -> Result<Response<ListTriggersResponse>, tonic::Status> {
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
            .trigger_aggregate_service()
            .list(&project_id)
            .await
            .context("list triggers")
            .to_internal_error()?;

        Ok(Response::new(ListTriggersResponse {
            triggers: recs.into_iter().map(record_to_grpc).collect(),
        }))
    }
}
