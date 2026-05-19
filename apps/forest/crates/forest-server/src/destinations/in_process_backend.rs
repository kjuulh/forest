use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Context;
use forest_runner::backend::{
    DestinationBackend, DestinationConfig, ProjectInfo, ReleaseAnnotation,
};
use sqlx::PgPool;

use crate::{
    destinations::logger::DestinationLogger,
    services::artifact_staging_registry::ArtifactStagingRegistry,
    temp_dir::{GuardedTempDirectory, TempDirectories},
};

/// In-process backend for destination handlers.
///
/// Uses the server's existing registries and database directly, allowing
/// the same destination handler code (from forest-runner) to run inside
/// the forest-server process without gRPC.
pub struct InProcessBackend {
    artifact_files: ArtifactStagingRegistry,
    db: PgPool,
    logger: DestinationLogger,
    temp: TempDirectories,
    artifact_id: uuid::Uuid,
    project_id: uuid::Uuid,
    environment: String,
    release_identity: Option<forest_runner::backend::ReleaseIdentity>,
    /// Keep temp directory guards alive for the lifetime of this backend.
    temp_guards: Mutex<Vec<GuardedTempDirectory>>,
}

impl InProcessBackend {
    pub fn new(
        artifact_files: ArtifactStagingRegistry,
        db: PgPool,
        logger: DestinationLogger,
        temp: TempDirectories,
        artifact_id: uuid::Uuid,
        project_id: uuid::Uuid,
        environment: String,
    ) -> Self {
        Self {
            artifact_files,
            db,
            logger,
            temp,
            artifact_id,
            project_id,
            environment,
            release_identity: None,
            temp_guards: Mutex::new(Vec::new()),
        }
    }

    pub fn with_release_identity(mut self, identity: forest_runner::backend::ReleaseIdentity) -> Self {
        self.release_identity = Some(identity);
        self
    }
}

impl InProcessBackend {
    /// Build a `DestinationConfig` from a `forest_models::Destination`.
    pub fn config_from_destination(dest: &forest_models::Destination) -> DestinationConfig {
        DestinationConfig {
            name: dest.name.clone(),
            environment: dest.environment.clone(),
            metadata: dest.metadata.clone(),
            organisation: dest.destination_type.organisation.clone(),
            type_name: dest.destination_type.name.clone(),
            type_version: dest.destination_type.version as u64,
        }
    }
}

#[async_trait::async_trait]
impl DestinationBackend for InProcessBackend {
    async fn get_deployment_files(&self) -> anyhow::Result<Vec<(PathBuf, String)>> {
        self.artifact_files
            .get_files_for_release(&self.artifact_id, &self.environment)
            .await
            .context("get deployment files")
    }

    async fn get_spec_files(&self) -> anyhow::Result<Vec<(PathBuf, String)>> {
        self.artifact_files
            .get_spec_files(&self.artifact_id)
            .await
            .context("get spec files")
    }

    async fn get_release_annotation(&self) -> anyhow::Result<ReleaseAnnotation> {
        let rec = sqlx::query!(
            "SELECT slug, source, context, ref, created FROM annotations WHERE artifact_id = $1",
            self.artifact_id
        )
        .fetch_one(&self.db)
        .await
        .context("get annotation for release metadata")?;

        fn json_str(val: &serde_json::Value, key: &str) -> Option<String> {
            val.get(key)
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        }

        let source = &rec.source;
        let context = &rec.context;
        let reference = &rec.r#ref;

        Ok(ReleaseAnnotation {
            slug: rec.slug,
            source_username: json_str(source, "username"),
            source_email: json_str(source, "email"),
            context_title: json_str(context, "title"),
            context_description: json_str(context, "description"),
            context_web: json_str(context, "web"),
            reference_version: json_str(reference, "version"),
            reference_commit_sha: json_str(reference, "commit_sha"),
            reference_commit_branch: json_str(reference, "commit_branch"),
            reference_commit_message: json_str(reference, "commit_message"),
            created_at: rec.created.to_rfc3339(),
        })
    }

    async fn get_project_info(&self) -> anyhow::Result<ProjectInfo> {
        let rec = sqlx::query!(
            "SELECT organisation, project FROM projects WHERE id = $1",
            self.project_id
        )
        .fetch_one(&self.db)
        .await
        .context("get project info")?;

        Ok(ProjectInfo {
            organisation: rec.organisation,
            project: rec.project,
        })
    }

    fn log_stdout(&self, line: &str) {
        self.logger.log_stdout(line);
    }

    fn log_stderr(&self, line: &str) {
        self.logger.log_stderr(line);
    }

    async fn create_temp_dir(&self) -> anyhow::Result<PathBuf> {
        let guarded = self.temp.create_emphemeral_temp().await?;
        // Deref<Target=PathBuf> gives us the path
        let path = (*guarded).clone();
        // Keep the guard alive so the temp dir isn't cleaned up
        self.temp_guards.lock().unwrap().push(guarded);
        Ok(path)
    }

    async fn get_release_identity(&self) -> Option<forest_runner::backend::ReleaseIdentity> {
        self.release_identity.clone()
    }
}
