pub mod remote;

use std::collections::HashMap;
use std::path::PathBuf;

/// Annotation context for a release, used to build `.forest/release.yaml` metadata.
#[derive(Debug, Clone)]
pub struct ReleaseAnnotation {
    pub slug: String,
    pub source_username: Option<String>,
    pub source_email: Option<String>,
    pub context_title: Option<String>,
    pub context_description: Option<String>,
    pub context_web: Option<String>,
    pub reference_version: Option<String>,
    pub reference_commit_sha: Option<String>,
    pub reference_commit_branch: Option<String>,
    pub reference_commit_message: Option<String>,
    pub created_at: String,
}

/// Project identification used for directory naming.
#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub organisation: String,
    pub project: String,
}

/// Destination configuration — decoupled from `forest_models::Destination`.
///
/// Can be constructed from the server's `Destination` model or from
/// the gRPC `DestinationInfo` message.
#[derive(Debug, Clone)]
pub struct DestinationConfig {
    pub name: String,
    pub environment: String,
    pub metadata: HashMap<String, String>,
    pub organisation: String,
    pub type_name: String,
    pub type_version: u64,
}

/// Identity metadata for a release, used to annotate kubernetes resources
/// so external agents can correlate cluster state back to forest releases.
#[derive(Debug, Clone, Default)]
pub struct ReleaseIdentity {
    pub release_intent_id: Option<String>,
    pub release_id: Option<String>,
    pub artifact_id: Option<String>,
    pub organisation: String,
    pub project: String,
    pub destination: String,
    pub environment: String,
}

/// Abstraction over the backing data + logging infrastructure.
///
/// Implementations are either:
/// - `InProcessBackend` (forest-server): reads DB, uses `DestinationLogger`
/// - `RemoteBackend` (forest-runner binary): uses pre-fetched data, `RemoteLogger`
#[async_trait::async_trait]
pub trait DestinationBackend: Send + Sync {
    /// Rendered deployment manifest files for this release+environment.
    async fn get_deployment_files(&self) -> anyhow::Result<Vec<(PathBuf, String)>>;

    /// Original spec files for this release.
    async fn get_spec_files(&self) -> anyhow::Result<Vec<(PathBuf, String)>>;

    /// Annotation context for the release.
    async fn get_release_annotation(&self) -> anyhow::Result<ReleaseAnnotation>;

    /// Project organisation and name, used for directory naming.
    async fn get_project_info(&self) -> anyhow::Result<ProjectInfo>;

    /// Release identity for annotating kubernetes resources.
    /// Returns None if identity info is not available (e.g. local prepare without server).
    async fn get_release_identity(&self) -> Option<ReleaseIdentity> {
        None
    }

    /// Log a line to stdout.
    fn log_stdout(&self, line: &str);

    /// Log a line to stderr.
    fn log_stderr(&self, line: &str);

    /// Create a temporary directory for scratch work.
    async fn create_temp_dir(&self) -> anyhow::Result<PathBuf>;
}
