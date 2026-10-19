pub mod remote;

use std::collections::HashMap;
use std::path::PathBuf;

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

#[derive(Debug, Clone)]
pub struct ProjectInfo {
    pub organisation: String,
    pub project: String,
}

#[derive(Debug, Clone)]
pub struct DestinationConfig {
    pub name: String,
    pub environment: String,
    pub metadata: HashMap<String, String>,
    pub organisation: String,
    pub type_name: String,
    pub type_version: u64,
}

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

#[async_trait::async_trait]
pub trait DestinationBackend: Send + Sync {
    async fn get_deployment_files(&self) -> anyhow::Result<Vec<(PathBuf, String)>>;

    async fn get_spec_files(&self) -> anyhow::Result<Vec<(PathBuf, String)>>;

    async fn get_release_annotation(&self) -> anyhow::Result<ReleaseAnnotation>;

    async fn get_project_info(&self) -> anyhow::Result<ProjectInfo>;

    /// `None` when identity is unavailable, e.g. a local prepare without a server.
    async fn get_release_identity(&self) -> Option<ReleaseIdentity> {
        None
    }

    fn log_stdout(&self, line: &str);

    fn log_stderr(&self, line: &str);

    async fn create_temp_dir(&self) -> anyhow::Result<PathBuf>;
}
