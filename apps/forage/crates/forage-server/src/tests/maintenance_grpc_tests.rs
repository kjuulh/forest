use std::collections::HashMap;
use std::sync::Arc;

use forage_core::compute::{
    ComputeError, ComputeInstance, ComputeResourceSpec, ComputeScheduler, Rollout, RolloutEvent,
};
use forage_grpc::forage_service_server::ForageService;
use forage_grpc::{ApplyResourcesRequest, DeleteResourcesRequest, WatchRolloutRequest};
use tokio_stream::StreamExt;
use tonic::Request;

use crate::compute_grpc::ForageServiceImpl;

struct RejectingScheduler;

#[async_trait::async_trait]
impl ComputeScheduler for RejectingScheduler {
    async fn apply_resources(
        &self,
        _apply_id: &str,
        _namespace: &str,
        _resources: Vec<ComputeResourceSpec>,
        _labels: HashMap<String, String>,
    ) -> Result<String, ComputeError> {
        Err(ComputeError::Internal("scheduler must not be called".into()))
    }

    async fn watch_rollout(
        &self,
        _rollout_id: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<RolloutEvent>, ComputeError> {
        Err(ComputeError::Internal("scheduler must not be called".into()))
    }

    async fn delete_resources(
        &self,
        _namespace: &str,
        _labels: HashMap<String, String>,
    ) -> Result<(), ComputeError> {
        Err(ComputeError::Internal("scheduler must not be called".into()))
    }

    async fn list_rollouts(&self, _namespace: &str) -> Result<Vec<Rollout>, ComputeError> {
        Err(ComputeError::Internal("scheduler must not be called".into()))
    }

    async fn get_rollout(&self, _rollout_id: &str) -> Result<Rollout, ComputeError> {
        Err(ComputeError::Internal("scheduler must not be called".into()))
    }

    async fn list_instances(
        &self,
        _namespace: &str,
    ) -> Result<Vec<ComputeInstance>, ComputeError> {
        Err(ComputeError::Internal("scheduler must not be called".into()))
    }
}

#[tokio::test]
async fn maintenance_grpc_returns_defaults_without_scheduling_work() {
    let service = ForageServiceImpl {
        scheduler: Arc::new(RejectingScheduler),
        maintenance_mode: true,
    };

    let apply = service
        .apply_resources(Request::new(ApplyResourcesRequest::default()))
        .await
        .unwrap()
        .into_inner();
    assert!(apply.rollout_id.is_empty());

    let mut events = service
        .watch_rollout(Request::new(WatchRolloutRequest::default()))
        .await
        .unwrap()
        .into_inner();
    assert!(events.next().await.is_none());

    service
        .delete_resources(Request::new(DeleteResourcesRequest::default()))
        .await
        .unwrap();
}
