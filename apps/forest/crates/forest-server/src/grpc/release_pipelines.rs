use anyhow::Context;
use forest_grpc_interface::{
    pipeline_stage, release_pipeline_service_server::ReleasePipelineService,
    DeployStageConfig, PipelineStage, PlanStageConfig, WaitStageConfig, *,
};
use tonic::Response;

use crate::{
    grpc::{artifacts::GrpcErrorExt, authorize},
    services::{
        release_pipeline::{
            CreatePipelineParams, PipelineStages, ReleasePipelineRegistryState,
            StageConfig, StageDefinition, UpdatePipelineParams,
        },
        release_registry::ReleaseRegistryState,
    },
    state::State,
};

pub struct ReleasePipelinesServer {
    pub state: State,
}

// ── Proto <-> Domain conversions ─────────────────────────────────────

fn stages_to_proto(stages: &PipelineStages) -> Vec<PipelineStage> {
    stages
        .iter()
        .map(|(id, def)| {
            let config = match &def.config {
                StageConfig::Deploy { environment } => {
                    Some(pipeline_stage::Config::Deploy(DeployStageConfig {
                        environment: environment.clone(),
                    }))
                }
                StageConfig::Wait { duration_seconds } => {
                    Some(pipeline_stage::Config::Wait(WaitStageConfig {
                        duration_seconds: *duration_seconds,
                    }))
                }
                StageConfig::Plan { environment, auto_approve } => {
                    Some(pipeline_stage::Config::Plan(PlanStageConfig {
                        environment: environment.clone(),
                        auto_approve: *auto_approve,
                    }))
                }
            };

            PipelineStage {
                id: id.clone(),
                depends_on: def.depends_on.clone(),
                config,
            }
        })
        .collect()
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
            None => anyhow::bail!("stage '{}' is missing a config (deploy, wait, or plan)", ps.id),
        };

        let def = StageDefinition {
            depends_on: ps.depends_on,
            config,
        };

        if stages.insert(ps.id.clone(), def).is_some() {
            anyhow::bail!("duplicate stage id '{}'", ps.id);
        }
    }
    Ok(stages)
}

fn record_to_grpc(
    r: crate::services::release_pipeline::PipelineRecord,
) -> anyhow::Result<ReleasePipeline> {
    let stages = r.parse_stages()?;
    Ok(ReleasePipeline {
        id: r.id.to_string(),
        name: r.name,
        enabled: r.enabled,
        stages: stages_to_proto(&stages),
        created_at: r.created_at.to_rfc3339(),
        updated_at: r.updated_at.to_rfc3339(),
    })
}

#[async_trait::async_trait]
impl ReleasePipelineService for ReleasePipelinesServer {
    async fn create_release_pipeline(
        &self,
        request: tonic::Request<CreateReleasePipelineRequest>,
    ) -> Result<Response<CreateReleasePipelineResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        authorize::require_project_access(
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

        let stages = stages_from_proto(req.stages)
            .context("invalid stages")
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
            pipeline: Some(record_to_grpc(rec).to_internal_error()?),
        }))
    }

    async fn update_release_pipeline(
        &self,
        request: tonic::Request<UpdateReleasePipelineRequest>,
    ) -> Result<Response<UpdateReleasePipelineResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        authorize::require_project_access(
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

        let stages = if req.update_stages {
            Some(
                stages_from_proto(req.stages)
                    .context("invalid stages")
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
            pipeline: Some(record_to_grpc(rec).to_internal_error()?),
        }))
    }

    async fn delete_release_pipeline(
        &self,
        request: tonic::Request<DeleteReleasePipelineRequest>,
    ) -> Result<Response<DeleteReleasePipelineResponse>, tonic::Status> {
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        authorize::require_project_access(
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
        let actor = authorize::extract_actor(&request)?;
        let req = request.into_inner();

        let project = req
            .project
            .context("project is required")
            .to_internal_error()?;

        authorize::require_project_access(
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
            .release_pipeline_registry()
            .list(&project_id)
            .await
            .context("list release pipelines")
            .to_internal_error()?;

        let pipelines = recs
            .into_iter()
            .map(record_to_grpc)
            .collect::<anyhow::Result<Vec<_>>>()
            .to_internal_error()?;

        Ok(Response::new(ListReleasePipelinesResponse { pipelines }))
    }
}
