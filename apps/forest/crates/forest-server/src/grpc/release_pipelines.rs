use anyhow::Context;
use forest_grpc_interface::{
    DeployStageConfig, GateStageConfig, GateTimeoutBehaviour, HealthStatus, PipelineStage,
    PlanStageConfig, SignalRequirement as ProtoSignalRequirement, WaitStageConfig, pipeline_stage,
    release_pipeline_service_server::ReleasePipelineService, *,
};
use tonic::Response;

use crate::{
    grpc::{artifacts::GrpcErrorExt, authorize},
    services::{
        release_pipeline::{
            CreatePipelineParams, GateTimeout, PipelineStages, ReleasePipelineRegistryState,
            SignalRequirement, StageConfig, StageDefinition, UpdatePipelineParams,
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
                StageConfig::Plan {
                    environment,
                    auto_approve,
                } => Some(pipeline_stage::Config::Plan(PlanStageConfig {
                    environment: environment.clone(),
                    auto_approve: *auto_approve,
                })),
                StageConfig::Gate {
                    requires,
                    timeout_seconds,
                    on_timeout,
                } => Some(pipeline_stage::Config::Gate(GateStageConfig {
                    requires: requirements_to_proto(requires),
                    timeout_seconds: *timeout_seconds,
                    on_timeout: gate_timeout_to_proto(on_timeout),
                })),
            };

            PipelineStage {
                id: id.clone(),
                depends_on: def.depends_on.clone(),
                config,
            }
        })
        .collect()
}

// ── Gate conversion, shared with org_rules ───────────────────────────────
//
// `stages_to_proto` / `stages_from_proto` are duplicated between this module
// and `org_rules`, which predates this change. Rather than add a third copy of
// the gate's own conversion to that, the gate-specific parts live here and both
// call them.

pub(crate) fn gate_timeout_from_proto(v: i32) -> GateTimeout {
    match GateTimeoutBehaviour::try_from(v) {
        Ok(GateTimeoutBehaviour::Proceed) => GateTimeout::Proceed,
        // Unspecified included: an unset value must mean Fail, or a gate whose
        // caller forgot the field would quietly stop gating.
        _ => GateTimeout::Fail,
    }
}

pub(crate) fn gate_timeout_to_proto(t: &GateTimeout) -> i32 {
    match t {
        GateTimeout::Fail => GateTimeoutBehaviour::Fail as i32,
        GateTimeout::Proceed => GateTimeoutBehaviour::Proceed as i32,
    }
}

pub(crate) fn requirements_from_proto(reqs: Vec<ProtoSignalRequirement>) -> Vec<SignalRequirement> {
    reqs.into_iter()
        .map(|r| SignalRequirement {
            signal: r.signal,
            accept: r
                .accept
                .into_iter()
                .filter_map(|s| match HealthStatus::try_from(s) {
                    Ok(HealthStatus::Healthy) => Some("HEALTHY".to_string()),
                    Ok(HealthStatus::Progressing) => Some("PROGRESSING".to_string()),
                    Ok(HealthStatus::Degraded) => Some("DEGRADED".to_string()),
                    Ok(HealthStatus::Unhealthy) => Some("UNHEALTHY".to_string()),
                    Ok(HealthStatus::Missing) => Some("MISSING".to_string()),
                    // Dropped rather than mapped to a default. An UNSPECIFIED
                    // in the accept list would otherwise become a status the
                    // gate waits for and nothing can send; dropping it leaves
                    // the list empty, which `accepted()` reads as HEALTHY.
                    Ok(HealthStatus::Unspecified) | Err(_) => None,
                })
                .collect(),
        })
        .collect()
}

pub(crate) fn requirements_to_proto(reqs: &[SignalRequirement]) -> Vec<ProtoSignalRequirement> {
    reqs.iter()
        .map(|r| ProtoSignalRequirement {
            signal: r.signal.clone(),
            accept: r
                .accept
                .iter()
                .map(|s| match s.as_str() {
                    "HEALTHY" => HealthStatus::Healthy as i32,
                    "PROGRESSING" => HealthStatus::Progressing as i32,
                    "DEGRADED" => HealthStatus::Degraded as i32,
                    "UNHEALTHY" => HealthStatus::Unhealthy as i32,
                    "MISSING" => HealthStatus::Missing as i32,
                    _ => HealthStatus::Unspecified as i32,
                })
                .collect(),
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
            Some(pipeline_stage::Config::Gate(c)) => StageConfig::Gate {
                requires: requirements_from_proto(c.requires),
                timeout_seconds: c.timeout_seconds,
                on_timeout: gate_timeout_from_proto(c.on_timeout),
            },
            None => anyhow::bail!(
                "stage '{}' is missing a config (deploy, wait, plan, or gate)",
                ps.id
            ),
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
