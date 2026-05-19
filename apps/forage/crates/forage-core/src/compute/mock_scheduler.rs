use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use uuid::Uuid;

use super::{
    ComputeError, ComputeInstance, ComputeResourceSpec, ComputeScheduler, ResourceKind, Rollout,
    RolloutEvent, RolloutResource, RolloutStatus,
};

struct MockState {
    rollouts: HashMap<String, Rollout>,
    instances: HashMap<String, Vec<ComputeInstance>>,
}

/// In-memory compute scheduler that simulates container lifecycle.
///
/// Stores rollouts and instances in memory.  When `apply_resources` is called
/// it spawns a background task that transitions each resource through
/// PENDING → IN_PROGRESS → SUCCEEDED with short delays.
pub struct InMemoryComputeScheduler {
    state: Arc<Mutex<MockState>>,
}

impl InMemoryComputeScheduler {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MockState {
                rollouts: HashMap::new(),
                instances: HashMap::new(),
            })),
        }
    }
}

impl Default for InMemoryComputeScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ComputeScheduler for InMemoryComputeScheduler {
    async fn apply_resources(
        &self,
        apply_id: &str,
        namespace: &str,
        resources: Vec<ComputeResourceSpec>,
        labels: HashMap<String, String>,
    ) -> Result<String, ComputeError> {
        let rollout_id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now();

        let rollout_resources: Vec<RolloutResource> = resources
            .iter()
            .map(|r| RolloutResource {
                name: r.name.clone(),
                kind: r.kind,
                status: RolloutStatus::Pending,
                message: "queued".into(),
            })
            .collect();

        let rollout = Rollout {
            id: rollout_id.clone(),
            apply_id: apply_id.to_string(),
            namespace: namespace.to_string(),
            resources: rollout_resources,
            status: RolloutStatus::Pending,
            labels: labels.clone(),
            created_at: now,
        };

        // Create instances for container-service resources
        let region = labels.get("region").cloned().unwrap_or("eu-west-1".into());
        let project = labels.get("project").cloned().unwrap_or_default();
        let destination = labels.get("destination").cloned().unwrap_or_default();
        let environment = labels.get("environment").cloned().unwrap_or_default();
        let new_instances: Vec<ComputeInstance> = resources
            .iter()
            .filter(|r| r.kind == ResourceKind::ContainerService)
            .map(|r| ComputeInstance {
                id: Uuid::new_v4().to_string(),
                namespace: namespace.to_string(),
                resource_name: r.name.clone(),
                project: project.clone(),
                destination: destination.clone(),
                environment: environment.clone(),
                region: region.clone(),
                image: r.image.clone().unwrap_or_else(|| "unknown".into()),
                replicas: r.replicas,
                cpu: r.cpu.clone().unwrap_or_else(|| "250m".into()),
                memory: r.memory.clone().unwrap_or_else(|| "256Mi".into()),
                status: "pending".into(),
                created_at: now,
            })
            .collect();

        {
            let mut state = self.state.lock().await;
            state.rollouts.insert(rollout_id.clone(), rollout);
            let ns_instances = state
                .instances
                .entry(namespace.to_string())
                .or_insert_with(Vec::new);
            // Upsert: replace existing instances with the same resource_name
            for new_inst in new_instances {
                if let Some(existing) = ns_instances
                    .iter_mut()
                    .find(|i| i.resource_name == new_inst.resource_name)
                {
                    *existing = new_inst;
                } else {
                    ns_instances.push(new_inst);
                }
            }
        }

        // Spawn background simulation
        let state = self.state.clone();
        let rid = rollout_id.clone();
        let ns = namespace.to_string();
        let resource_names: Vec<(String, String)> = resources
            .iter()
            .map(|r| (r.name.clone(), r.kind.to_string()))
            .collect();

        tokio::spawn(async move {
            // Transition to InProgress
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            {
                let mut state = state.lock().await;
                if let Some(rollout) = state.rollouts.get_mut(&rid) {
                    rollout.status = RolloutStatus::InProgress;
                    for r in &mut rollout.resources {
                        r.status = RolloutStatus::InProgress;
                        r.message = "deploying".into();
                    }
                }
                // Update instance statuses
                if let Some(instances) = state.instances.get_mut(&ns) {
                    for inst in instances.iter_mut() {
                        if inst.status == "pending" {
                            inst.status = "running".into();
                        }
                    }
                }
            }

            // Simulate per-resource completion
            for (name, _kind) in &resource_names {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                let mut state = state.lock().await;
                if let Some(rollout) = state.rollouts.get_mut(&rid) {
                    if let Some(r) = rollout.resources.iter_mut().find(|r| &r.name == name) {
                        r.status = RolloutStatus::Succeeded;
                        r.message = "ready".into();
                    }
                }
            }

            // Mark rollout as succeeded
            {
                let mut state = state.lock().await;
                if let Some(rollout) = state.rollouts.get_mut(&rid) {
                    rollout.status = RolloutStatus::Succeeded;
                }
            }
        });

        Ok(rollout_id)
    }

    async fn watch_rollout(
        &self,
        rollout_id: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<RolloutEvent>, ComputeError> {
        let rollout = {
            let state = self.state.lock().await;
            state
                .rollouts
                .get(rollout_id)
                .cloned()
                .ok_or_else(|| ComputeError::NotFound(format!("rollout {rollout_id}")))?
        };

        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let state = self.state.clone();
        let rid = rollout_id.to_string();
        let resource_specs: Vec<(String, String)> = rollout
            .resources
            .iter()
            .map(|r| (r.name.clone(), r.kind.to_string()))
            .collect();

        tokio::spawn(async move {
            // Emit pending events
            for (name, kind) in &resource_specs {
                let _ = tx
                    .send(RolloutEvent {
                        resource_name: name.clone(),
                        resource_kind: kind.clone(),
                        status: RolloutStatus::Pending,
                        message: "queued".into(),
                    })
                    .await;
            }

            // Poll until the rollout is done
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                let state = state.lock().await;
                let Some(rollout) = state.rollouts.get(&rid) else {
                    break;
                };

                for r in &rollout.resources {
                    let _ = tx
                        .send(RolloutEvent {
                            resource_name: r.name.clone(),
                            resource_kind: r.kind.to_string(),
                            status: r.status,
                            message: r.message.clone(),
                        })
                        .await;
                }

                if matches!(
                    rollout.status,
                    RolloutStatus::Succeeded | RolloutStatus::Failed | RolloutStatus::RolledBack
                ) {
                    break;
                }
            }
        });

        Ok(rx)
    }

    async fn delete_resources(
        &self,
        namespace: &str,
        labels: HashMap<String, String>,
    ) -> Result<(), ComputeError> {
        let mut state = self.state.lock().await;

        // Remove matching rollouts
        state.rollouts.retain(|_, r| {
            if r.namespace != namespace {
                return true;
            }
            for (k, v) in &labels {
                if r.labels.get(k) != Some(v) {
                    return true;
                }
            }
            false
        });

        // Remove matching instances
        if let Some(instances) = state.instances.get_mut(namespace) {
            if labels.is_empty() {
                instances.clear();
            }
            // If labels are specified we'd filter more precisely, but for now
            // the mock just clears the namespace.
        }

        Ok(())
    }

    async fn list_rollouts(&self, namespace: &str) -> Result<Vec<Rollout>, ComputeError> {
        let state = self.state.lock().await;
        let mut rollouts: Vec<Rollout> = state
            .rollouts
            .values()
            .filter(|r| r.namespace == namespace)
            .cloned()
            .collect();
        rollouts.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(rollouts)
    }

    async fn get_rollout(&self, rollout_id: &str) -> Result<Rollout, ComputeError> {
        let state = self.state.lock().await;
        state
            .rollouts
            .get(rollout_id)
            .cloned()
            .ok_or_else(|| ComputeError::NotFound(format!("rollout {rollout_id}")))
    }

    async fn list_instances(
        &self,
        namespace: &str,
    ) -> Result<Vec<ComputeInstance>, ComputeError> {
        let state = self.state.lock().await;
        Ok(state
            .instances
            .get(namespace)
            .cloned()
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn apply_creates_rollout_and_instances() {
        let scheduler = InMemoryComputeScheduler::new();

        let resources = vec![ComputeResourceSpec {
            name: "my-api".into(),
            kind: ResourceKind::ContainerService,
            image: Some("registry.forage.sh/org/app:v1".into()),
            replicas: 2,
            cpu: Some("500m".into()),
            memory: Some("512Mi".into()),
        }];

        let mut labels = HashMap::new();
        labels.insert("region".into(), "eu-west-1".into());

        let rollout_id = scheduler
            .apply_resources("test-apply-1", "test-ns", resources, labels)
            .await
            .unwrap();

        assert!(!rollout_id.is_empty());

        // Rollout should exist
        let rollout = scheduler.get_rollout(&rollout_id).await.unwrap();
        assert_eq!(rollout.namespace, "test-ns");
        assert_eq!(rollout.resources.len(), 1);

        // Instance should exist
        let instances = scheduler.list_instances("test-ns").await.unwrap();
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].image, "registry.forage.sh/org/app:v1");
        assert_eq!(instances[0].replicas, 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rollout_completes_successfully() {
        let scheduler = InMemoryComputeScheduler::new();

        let resources = vec![ComputeResourceSpec {
            name: "svc".into(),
            kind: ResourceKind::ContainerService,
            image: Some("img:latest".into()),
            replicas: 1,
            cpu: None,
            memory: None,
        }];

        let rollout_id = scheduler
            .apply_resources("test-2", "ns", resources, HashMap::new())
            .await
            .unwrap();

        // Wait for simulation to complete
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let rollout = scheduler.get_rollout(&rollout_id).await.unwrap();
        assert_eq!(rollout.status, RolloutStatus::Succeeded);
        assert_eq!(rollout.resources[0].status, RolloutStatus::Succeeded);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn watch_rollout_streams_events() {
        let scheduler = InMemoryComputeScheduler::new();

        let resources = vec![ComputeResourceSpec {
            name: "app".into(),
            kind: ResourceKind::ContainerService,
            image: Some("img:v1".into()),
            replicas: 1,
            cpu: None,
            memory: None,
        }];

        let rollout_id = scheduler
            .apply_resources("test-3", "ns", resources, HashMap::new())
            .await
            .unwrap();

        let mut rx = scheduler.watch_rollout(&rollout_id).await.unwrap();

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
            if events.last().map(|e| e.status) == Some(RolloutStatus::Succeeded) {
                break;
            }
        }

        assert!(!events.is_empty());
        // Should have at least pending + succeeded
        assert!(events.iter().any(|e| e.status == RolloutStatus::Pending));
        assert!(events.iter().any(|e| e.status == RolloutStatus::Succeeded));
    }

    #[tokio::test]
    async fn delete_removes_resources() {
        let scheduler = InMemoryComputeScheduler::new();

        let resources = vec![ComputeResourceSpec {
            name: "app".into(),
            kind: ResourceKind::ContainerService,
            image: Some("img:v1".into()),
            replicas: 1,
            cpu: None,
            memory: None,
        }];

        let mut labels = HashMap::new();
        labels.insert("project".into(), "test".into());

        scheduler
            .apply_resources("del-1", "ns", resources, labels.clone())
            .await
            .unwrap();

        assert_eq!(scheduler.list_rollouts("ns").await.unwrap().len(), 1);

        scheduler.delete_resources("ns", labels).await.unwrap();

        assert_eq!(scheduler.list_rollouts("ns").await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn watch_nonexistent_rollout_returns_not_found() {
        let scheduler = InMemoryComputeScheduler::new();
        let result = scheduler.watch_rollout("does-not-exist").await;
        assert!(matches!(result, Err(ComputeError::NotFound(_))));
    }
}
