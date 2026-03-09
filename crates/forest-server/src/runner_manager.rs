use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use forest_grpc_interface::WorkAssignment;
use tokio::sync::{RwLock, mpsc};

use crate::destinations::DestinationIndex;

/// Tracks connected runners and assigns work to them based on capabilities.
#[derive(Clone)]
pub struct RunnerManager {
    inner: Arc<RwLock<RunnerManagerInner>>,
}

struct RunnerManagerInner {
    runners: HashMap<String, ConnectedRunner>,
}

struct ConnectedRunner {
    capabilities: Vec<DestinationCapability>,
    max_concurrent: i32,
    active_releases: i32,
    work_sender: mpsc::Sender<WorkAssignment>,
    last_heartbeat: Instant,
}

#[derive(Debug, Clone)]
pub struct DestinationCapability {
    pub organisation: String,
    pub name: String,
    pub version: usize,
}

impl DestinationCapability {
    pub fn matches(&self, index: &DestinationIndex) -> bool {
        self.organisation == index.organisation
            && self.name == index.name
            && self.version == index.version
    }
}

impl Default for RunnerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RunnerManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RunnerManagerInner {
                runners: HashMap::new(),
            })),
        }
    }

    /// Register a new runner. Called when a runner sends RunnerRegister on the stream.
    pub async fn register_runner(
        &self,
        runner_id: String,
        capabilities: Vec<DestinationCapability>,
        max_concurrent: i32,
        work_sender: mpsc::Sender<WorkAssignment>,
    ) {
        let mut inner = self.inner.write().await;
        tracing::info!(
            runner_id = %runner_id,
            capabilities = ?capabilities.iter().map(|c| format!("{}/{}@{}", c.organisation, c.name, c.version)).collect::<Vec<_>>(),
            max_concurrent,
            "runner registered"
        );
        inner.runners.insert(
            runner_id,
            ConnectedRunner {
                capabilities,
                max_concurrent,
                active_releases: 0,
                work_sender,
                last_heartbeat: Instant::now(),
            },
        );
    }

    /// Unregister a runner. Called when the stream drops or the runner disconnects.
    /// Returns the runner_id if it was found.
    pub async fn unregister_runner(&self, runner_id: &str) -> bool {
        let mut inner = self.inner.write().await;
        let removed = inner.runners.remove(runner_id).is_some();
        if removed {
            tracing::info!(runner_id = %runner_id, "runner unregistered");
        }
        removed
    }

    /// Update heartbeat timestamp for a runner.
    pub async fn update_heartbeat(&self, runner_id: &str, active_releases: i32) {
        let mut inner = self.inner.write().await;
        if let Some(runner) = inner.runners.get_mut(runner_id) {
            runner.last_heartbeat = Instant::now();
            runner.active_releases = active_releases;
        }
    }

    /// Try to find a capable runner with spare capacity for the given destination type.
    /// If found, increments active_releases and returns (runner_id, work_sender).
    pub async fn try_assign(
        &self,
        dest_type: &DestinationIndex,
    ) -> Option<(String, mpsc::Sender<WorkAssignment>)> {
        let mut inner = self.inner.write().await;

        // Find a runner that:
        // 1. Has a matching capability
        // 2. Has spare capacity (active_releases < max_concurrent)
        // Pick the one with the most spare capacity (simple load balancing)
        let best = inner
            .runners
            .iter_mut()
            .filter(|(_, r)| {
                r.active_releases < r.max_concurrent
                    && r.capabilities.iter().any(|c| c.matches(dest_type))
            })
            .max_by_key(|(_, r)| r.max_concurrent - r.active_releases);

        if let Some((runner_id, runner)) = best {
            runner.active_releases += 1;
            let runner_id = runner_id.clone();
            let sender = runner.work_sender.clone();
            tracing::debug!(
                runner_id = %runner_id,
                dest_type = %dest_type,
                active_releases = runner.active_releases,
                "assigned work to runner"
            );
            Some((runner_id, sender))
        } else {
            None
        }
    }

    /// Decrement active_releases for a runner (called when a release completes).
    pub async fn release_completed(&self, runner_id: &str) {
        let mut inner = self.inner.write().await;
        if let Some(runner) = inner.runners.get_mut(runner_id) {
            runner.active_releases = (runner.active_releases - 1).max(0);
        }
    }

    /// Check for stale runners (no heartbeat for > threshold) and remove them.
    /// Returns the list of removed runner IDs.
    pub async fn reap_stale(&self, threshold: Duration) -> Vec<String> {
        let mut inner = self.inner.write().await;
        let now = Instant::now();
        let stale: Vec<String> = inner
            .runners
            .iter()
            .filter(|(_, r)| now.duration_since(r.last_heartbeat) > threshold)
            .map(|(id, _)| id.clone())
            .collect();

        for id in &stale {
            inner.runners.remove(id);
            tracing::warn!(runner_id = %id, "reaped stale runner (no heartbeat)");
        }

        stale
    }

    /// Returns whether any runners are currently connected.
    pub async fn has_runners(&self) -> bool {
        let inner = self.inner.read().await;
        !inner.runners.is_empty()
    }
}
