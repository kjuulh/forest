use std::pin::Pin;
use std::sync::Arc;

use forage_core::compute::{
    ComputeError, ComputeResourceSpec, ComputeScheduler, ResourceKind, RolloutStatus,
};
use forage_grpc::forage_service_server::ForageService;
use forage_grpc::{
    ApplyResourcesRequest, ApplyResourcesResponse, DeleteResourcesRequest,
    DeleteResourcesResponse, RolloutEvent as ProtoRolloutEvent, WatchRolloutRequest,
};
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

/// Implements the `ForageService` gRPC server trait.
///
/// Thin adapter: validates auth, converts proto to domain types, delegates to
/// the `ComputeScheduler`, and converts results back to proto.
pub struct ForageServiceImpl {
    pub scheduler: Arc<dyn ComputeScheduler>,
}

type WatchStream =
    Pin<Box<dyn Stream<Item = Result<ProtoRolloutEvent, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl ForageService for ForageServiceImpl {
    async fn apply_resources(
        &self,
        request: Request<ApplyResourcesRequest>,
    ) -> Result<Response<ApplyResourcesResponse>, Status> {
        let req = request.into_inner();

        if req.namespace.is_empty() {
            return Err(Status::invalid_argument("namespace is required"));
        }
        if req.resources.is_empty() {
            return Err(Status::invalid_argument("at least one resource is required"));
        }

        let resources: Vec<ComputeResourceSpec> = req
            .resources
            .iter()
            .map(|r| {
                let (kind, image, replicas, cpu, memory) = match &r.spec {
                    Some(forage_grpc::forage_resource::Spec::ContainerService(cs)) => {
                        let container = cs.container.as_ref();
                        let scaling = cs.scaling.as_ref();
                        (
                            ResourceKind::ContainerService,
                            container.map(|c| c.image.clone()),
                            scaling.map(|s| s.replicas).unwrap_or(1),
                            container
                                .and_then(|c| c.resources.as_ref())
                                .and_then(|r| r.requests.as_ref())
                                .map(|r| r.cpu.clone()),
                            container
                                .and_then(|c| c.resources.as_ref())
                                .and_then(|r| r.requests.as_ref())
                                .map(|r| r.memory.clone()),
                        )
                    }
                    Some(forage_grpc::forage_resource::Spec::Service(_)) => {
                        (ResourceKind::Service, None, 1, None, None)
                    }
                    Some(forage_grpc::forage_resource::Spec::Route(_)) => {
                        (ResourceKind::Route, None, 1, None, None)
                    }
                    Some(forage_grpc::forage_resource::Spec::CronJob(cj)) => {
                        let image = cj.container.as_ref().map(|c| c.image.clone());
                        (ResourceKind::CronJob, image, 1, None, None)
                    }
                    Some(forage_grpc::forage_resource::Spec::Job(j)) => {
                        let image = j.container.as_ref().map(|c| c.image.clone());
                        (ResourceKind::Job, image, 1, None, None)
                    }
                    None => (ResourceKind::ContainerService, None, 1, None, None),
                };

                ComputeResourceSpec {
                    name: r.name.clone(),
                    kind,
                    image,
                    replicas,
                    cpu,
                    memory,
                }
            })
            .collect();

        let rollout_id = self
            .scheduler
            .apply_resources(&req.apply_id, &req.namespace, resources, req.labels)
            .await
            .map_err(compute_err_to_status)?;

        Ok(Response::new(ApplyResourcesResponse { rollout_id }))
    }

    type WatchRolloutStream = WatchStream;

    async fn watch_rollout(
        &self,
        request: Request<WatchRolloutRequest>,
    ) -> Result<Response<Self::WatchRolloutStream>, Status> {
        let rollout_id = request.into_inner().rollout_id;

        let mut rx = self
            .scheduler
            .watch_rollout(&rollout_id)
            .await
            .map_err(compute_err_to_status)?;

        let stream = async_stream::stream! {
            while let Some(event) = rx.recv().await {
                yield Ok(ProtoRolloutEvent {
                    resource_name: event.resource_name,
                    resource_kind: event.resource_kind,
                    status: domain_status_to_proto(event.status) as i32,
                    message: event.message,
                });
            }
        };

        Ok(Response::new(Box::pin(stream) as Self::WatchRolloutStream))
    }

    async fn delete_resources(
        &self,
        request: Request<DeleteResourcesRequest>,
    ) -> Result<Response<DeleteResourcesResponse>, Status> {
        let req = request.into_inner();

        self.scheduler
            .delete_resources(&req.namespace, req.labels)
            .await
            .map_err(compute_err_to_status)?;

        Ok(Response::new(DeleteResourcesResponse {}))
    }
}

fn compute_err_to_status(e: ComputeError) -> Status {
    match e {
        ComputeError::NotFound(msg) => Status::not_found(msg),
        ComputeError::InvalidRequest(msg) => Status::invalid_argument(msg),
        ComputeError::Conflict(msg) => Status::already_exists(msg),
        ComputeError::Internal(msg) => Status::internal(msg),
    }
}

fn domain_status_to_proto(s: RolloutStatus) -> forage_grpc::RolloutStatus {
    match s {
        RolloutStatus::Pending => forage_grpc::RolloutStatus::Pending,
        RolloutStatus::InProgress => forage_grpc::RolloutStatus::InProgress,
        RolloutStatus::Succeeded => forage_grpc::RolloutStatus::Succeeded,
        RolloutStatus::Failed => forage_grpc::RolloutStatus::Failed,
        RolloutStatus::RolledBack => forage_grpc::RolloutStatus::RolledBack,
    }
}
