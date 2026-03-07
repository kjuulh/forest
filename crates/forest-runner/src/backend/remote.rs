use std::path::PathBuf;

use super::{DestinationBackend, ProjectInfo, ReleaseAnnotation};
use crate::logger::RemoteLogger;

/// Backend backed by pre-fetched gRPC data and a `RemoteLogger`.
///
/// All data is fetched by the executor before the destination handler runs,
/// so the trait methods simply return cloned data.
pub struct RemoteBackend {
    deployment_files: Vec<(PathBuf, String)>,
    spec_files: Vec<(PathBuf, String)>,
    annotation: ReleaseAnnotation,
    project_info: ProjectInfo,
    logger: RemoteLogger,
    temp_dir: PathBuf,
}

impl RemoteBackend {
    pub fn new(
        deployment_files: Vec<(PathBuf, String)>,
        spec_files: Vec<(PathBuf, String)>,
        annotation: ReleaseAnnotation,
        project_info: ProjectInfo,
        logger: RemoteLogger,
        temp_dir: PathBuf,
    ) -> Self {
        Self {
            deployment_files,
            spec_files,
            annotation,
            project_info,
            logger,
            temp_dir,
        }
    }
}

#[async_trait::async_trait]
impl DestinationBackend for RemoteBackend {
    async fn get_deployment_files(&self) -> anyhow::Result<Vec<(PathBuf, String)>> {
        Ok(self.deployment_files.clone())
    }

    async fn get_spec_files(&self) -> anyhow::Result<Vec<(PathBuf, String)>> {
        Ok(self.spec_files.clone())
    }

    async fn get_release_annotation(&self) -> anyhow::Result<ReleaseAnnotation> {
        Ok(self.annotation.clone())
    }

    async fn get_project_info(&self) -> anyhow::Result<ProjectInfo> {
        Ok(self.project_info.clone())
    }

    fn log_stdout(&self, line: &str) {
        self.logger.log_stdout(line);
    }

    fn log_stderr(&self, line: &str) {
        self.logger.log_stderr(line);
    }

    async fn create_temp_dir(&self) -> anyhow::Result<PathBuf> {
        let dir = self.temp_dir.join(format!("work-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await?;
        Ok(dir)
    }
}
