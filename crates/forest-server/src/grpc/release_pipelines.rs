use anyhow::Context;
use forest_grpc_interface::{release_pipeline_service_server::ReleasePipelineService, *};
use tonic::Response;

use crate::{
    grpc::artifacts::GrpcErrorExt,
    services::{
        release_pipeline::{
            CreatePipelineParams, PipelineStages, ReleasePipelineRegistryState, UpdatePipelineParams,
        },
        release_registry::ReleaseRegistryState,
    },
    state::State,
};

pub struct ReleasePipelinesServer {
    pub state: State,
}

fn record_to_grpc(
    r: crate::services::release_pipeline::PipelineRecord,
) -> ReleasePipeline {
    ReleasePipeline {
        id: r.id.to_string(),
        name: r.name,
        enabled: r.enabled,
        stages_json: r.stages.to_string(),
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
    }
}

#[async_trait::async_trait]
impl ReleasePipelineService for ReleasePipelinesServer {
    async fn create_release_pipeline(
        &self,
        request: tonic::Request<CreateReleasePipelineRequest>,
    ) -> Result<Response<CreateReleasePipelineResponse>, tonic::Status> {
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

        let stages: PipelineStages = serde_json::from_str(&req.stages_json)
            .context("invalid stages_json")
            .to_internal_error()?;

        let rec = self
            .state
            .release_pipeline_registry()
            .create(CreatePipelineParams {
                project_id,
                name: req.name,
                stages,
            })
            .await
            .context("create release pipeline")
            .to_internal_error()?;

        Ok(Response::new(CreateReleasePipelineResponse {
            pipeline: Some(record_to_grpc(rec)),
        }))
    }

    async fn update_release_pipeline(
        &self,
        request: tonic::Request<UpdateReleasePipelineRequest>,
    ) -> Result<Response<UpdateReleasePipelineResponse>, tonic::Status> {
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

        let stages = if let Some(json) = req.stages_json {
            Some(
                serde_json::from_str::<PipelineStages>(&json)
                    .context("invalid stages_json")
                    .to_internal_error()?,
            )
        } else {
            None
        };

        let rec = self
            .state
            .release_pipeline_registry()
            .update(
                &project_id,
                &req.name,
                UpdatePipelineParams {
                    enabled: req.enabled,
                    stages,
                },
            )
            .await
            .context("update release pipeline")
            .to_internal_error()?;

        Ok(Response::new(UpdateReleasePipelineResponse {
            pipeline: Some(record_to_grpc(rec)),
        }))
    }

    async fn delete_release_pipeline(
        &self,
        request: tonic::Request<DeleteReleasePipelineRequest>,
    ) -> Result<Response<DeleteReleasePipelineResponse>, tonic::Status> {
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
            .release_pipeline_registry()
            .delete(&project_id, &req.name)
            .await
            .context("delete release pipeline")
            .to_internal_error()?;

        Ok(Response::new(DeleteReleasePipelineResponse {}))
    }

    async fn list_release_pipelines(
        &self,
        request: tonic::Request<ListReleasePipelinesRequest>,
    ) -> Result<Response<ListReleasePipelinesResponse>, tonic::Status> {
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
            .release_pipeline_registry()
            .list(&project_id)
            .await
            .context("list release pipelines")
            .to_internal_error()?;

        Ok(Response::new(ListReleasePipelinesResponse {
            pipelines: recs.into_iter().map(record_to_grpc).collect(),
        }))
    }
}
