use std::path::Path;

use anyhow::Context;
use minijinja::context;
use tokio::io::AsyncWriteExt;

use crate::state::State;

pub struct InitProject {}

const FILE_NAME: &str = "non.ron";

impl InitProject {
    pub async fn init(&self, path: &Path, project_name: &str) -> anyhow::Result<()> {
        let path = if path.is_dir() {
            path.to_path_buf()
        } else if path.is_file() {
            path.parent().map(|p| p.to_path_buf()).unwrap_or_default()
        } else {
            anyhow::bail!("path is neither a file or dir: {}", path.display())
        };

        let file_name = path.join(FILE_NAME);
        if let Some(parent) = file_name.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut file = tokio::fs::File::create_new(file_name)
            .await
            .context("failed to create file")?;

        file.write_all(&self.init_project(project_name)?)
            .await
            .context("failed to write to non.ron file")?;
        file.flush().await?;

        Ok(())
    }

    pub fn init_project(&self, project_name: &str) -> anyhow::Result<Vec<u8>> {
        self.init_file().with_project_name(project_name).build()
    }

    fn init_file(&self) -> InitFile {
        InitFile::default()
    }
}

#[derive(Default)]
struct InitFile {
    project_name: Option<String>,
}

const DEFAULT_INIT_RON: &str = r#"
NonProject(
    name: "{{ project_name }}"
)
"#;

impl InitFile {
    fn with_project_name(mut self, name: impl Into<String>) -> Self {
        self.project_name = Some(name.into());

        self
    }

    fn build(&self) -> anyhow::Result<Vec<u8>> {
        let mut env = minijinja::Environment::new();

        env.add_global("project_name", self.project_name.clone());

        let template = env
            .template_from_str(DEFAULT_INIT_RON)
            .context("load init template")?;

        let res = template.render(context! {}).context("render template")?;

        Ok(res.into_bytes())
    }
}

pub trait InitProjectState {
    fn init_project(&self) -> InitProject;
}

impl InitProjectState for State {
    fn init_project(&self) -> InitProject {
        InitProject {}
    }
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use crate::{features::init_project::InitProject, services::project::NonProject};

    #[test]
    fn can_create_init_file() -> anyhow::Result<()> {
        let expected = r#"
NonProject(
    name: "some-name"
)"#;

        let init = InitProject {};

        let output = init.init_project("some-name")?;

        let output_str = std::str::from_utf8(&output)?;

        pretty_assertions::assert_eq!(expected, output_str);

        let project: NonProject = ron::from_str(output_str)?;

        assert_eq!(
            NonProject {
                name: "some-name".into(),
                dependencies: BTreeMap::default(),
            },
            project
        );

        Ok(())
    }
}
