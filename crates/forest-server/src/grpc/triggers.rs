use anyhow::Context;
use forest_grpc_interface::{trigger_service_server::TriggerService, *};
use tonic::Response;

use crate::{
    grpc::artifacts::GrpcErrorExt,
    services::{
        trigger::{CreateTriggerParams, TriggerRegistryState, UpdateTriggerParams},
        event_bus::{EventBusState, EventPayload},
        release_registry::ReleaseRegistryState,
    },
    state::State,
};

pub struct TriggersServer {
    pub state: State,
}

fn record_to_grpc(r: crate::services::trigger::TriggerRecord) -> Trigger {
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
            .trigger_registry()
            .create(CreateTriggerParams {
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
            .trigger_registry()
            .update(
                &project_id,
                &req.name,
                UpdateTriggerParams {
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
            .trigger_registry()
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
            .trigger_registry()
            .list(&project_id)
            .await
            .context("list triggers")
            .to_internal_error()?;

        Ok(Response::new(ListTriggersResponse {
            triggers: recs.into_iter().map(record_to_grpc).collect(),
        }))
    }
}
