use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Region catalog
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Region {
    pub id: &'static str,
    pub name: &'static str,
    pub display_name: &'static str,
    pub available: bool,
}

pub const REGIONS: &[Region] = &[
    Region {
        id: "eu-west-1",
        name: "Europe (Ireland)",
        display_name: "eu-west-1 — Europe (Ireland)",
        available: true,
    },
    Region {
        id: "us-east-1",
        name: "US East (Virginia)",
        display_name: "us-east-1 — US East (Virginia)",
        available: true,
    },
    Region {
        id: "ap-southeast-1",
        name: "Asia Pacific (Singapore)",
        display_name: "ap-southeast-1 — Asia Pacific (Singapore)",
        available: false,
    },
];

pub fn available_regions() -> Vec<&'static Region> {
    REGIONS.iter().filter(|r| r.available).collect()
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ComputeResourceSpec {
    pub name: String,
    pub kind: ResourceKind,
    pub image: Option<String>,
    pub replicas: u32,
    pub cpu: Option<String>,
    pub memory: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    ContainerService,
    Service,
    Route,
    CronJob,
    Job,
}

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceKind::ContainerService => write!(f, "container_service"),
            ResourceKind::Service => write!(f, "service"),
            ResourceKind::Route => write!(f, "route"),
            ResourceKind::CronJob => write!(f, "cron_job"),
            ResourceKind::Job => write!(f, "job"),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Rollout {
    pub id: String,
    pub apply_id: String,
    pub namespace: String,
    pub resources: Vec<RolloutResource>,
    pub status: RolloutStatus,
    pub labels: HashMap<String, String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RolloutResource {
    pub name: String,
    pub kind: ResourceKind,
    pub status: RolloutStatus,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutStatus {
    Pending,
    InProgress,
    Succeeded,
    Failed,
    RolledBack,
}

impl std::fmt::Display for RolloutStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RolloutStatus::Pending => write!(f, "pending"),
            RolloutStatus::InProgress => write!(f, "in_progress"),
            RolloutStatus::Succeeded => write!(f, "succeeded"),
            RolloutStatus::Failed => write!(f, "failed"),
            RolloutStatus::RolledBack => write!(f, "rolled_back"),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RolloutEvent {
    pub resource_name: String,
    pub resource_kind: String,
    pub status: RolloutStatus,
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ComputeInstance {
    pub id: String,
    pub namespace: String,
    pub resource_name: String,
    pub project: String,
    pub destination: String,
    pub environment: String,
    pub region: String,
    pub image: String,
    pub replicas: u32,
    pub cpu: String,
    pub memory: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, thiserror::Error)]
pub enum ComputeError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("resource conflict: {0}")]
    Conflict(String),
    #[error("scheduler error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Scheduler trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait ComputeScheduler: Send + Sync {
    /// Apply a batch of resources. Returns a rollout ID for tracking.
    async fn apply_resources(
        &self,
        apply_id: &str,
        namespace: &str,
        resources: Vec<ComputeResourceSpec>,
        labels: HashMap<String, String>,
    ) -> Result<String, ComputeError>;

    /// Subscribe to rollout status events.
    async fn watch_rollout(
        &self,
        rollout_id: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<RolloutEvent>, ComputeError>;

    /// Delete resources by namespace + labels.
    async fn delete_resources(
        &self,
        namespace: &str,
        labels: HashMap<String, String>,
    ) -> Result<(), ComputeError>;

    /// List rollouts for a namespace.
    async fn list_rollouts(&self, namespace: &str) -> Result<Vec<Rollout>, ComputeError>;

    /// Get a specific rollout by ID.
    async fn get_rollout(&self, rollout_id: &str) -> Result<Rollout, ComputeError>;

    /// List running compute instances for a namespace.
    async fn list_instances(&self, namespace: &str) -> Result<Vec<ComputeInstance>, ComputeError>;
}

// ---------------------------------------------------------------------------
// In-memory mock scheduler
// ---------------------------------------------------------------------------

pub mod mock_scheduler;
pub use mock_scheduler::InMemoryComputeScheduler;
