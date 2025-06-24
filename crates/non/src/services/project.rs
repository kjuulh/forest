use std::path::Path;

use crate::state::State;

use anyhow::Context;
pub mod models;
pub use models::*;

const NON_PROJECT_FILE: &str = "non.toml";

pub struct ProjectParser {}

impl ProjectParser {
    pub async fn get_project(&self) -> anyhow::Result<Project> {
        let current_dir =
            std::env::current_dir().context("current project dir is required for a project")?;

        let project_file_content = self.find_project_file(current_dir).await?;

        let project_file: ProjectFile = toml::from_str(&project_file_content)?;

        Ok(project_file.project)
    }

    #[tracing::instrument(skip(self), level = "trace")]
    async fn find_project_file(&self, current_dir: std::path::PathBuf) -> anyhow::Result<String> {
        let mut dir_path = current_dir;

        loop {
            match self.get_project_file(&dir_path).await? {
                Some(output) => return Ok(output),
                None => {
                    if !dir_path.pop() {
                        anyhow::bail!("failed to find non.toml in project");
                    }
                }
            }
        }
    }

    #[tracing::instrument(skip(self), level = "debug")]
    async fn get_project_file(&self, dir: &Path) -> anyhow::Result<Option<String>> {
        let file_path = dir.join(NON_PROJECT_FILE);
        if !file_path.exists() {
            tracing::debug!("project file doesn't exist");
            return Ok(None);
        }

        let file_content = tokio::fs::read_to_string(&file_path)
            .await
            .context(format!("failed to read file: {}", file_path.display()))?;

        return Ok(Some(file_content));
    }
}

pub trait ProjectParserState {
    fn project_parser(&self) -> ProjectParser;
}

impl ProjectParserState for State {
    fn project_parser(&self) -> ProjectParser {
        ProjectParser {}
    }
}
