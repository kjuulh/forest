use std::path::PathBuf;

use shell::ShellExecutor;

use crate::model::{Context, Script};

pub mod shell;

#[derive(Clone)]
pub struct ScriptExecutor {
    project_path: PathBuf,
    ctx: Context,
}

impl ScriptExecutor {
    pub fn new(project_path: PathBuf, ctx: Context) -> Self {
        Self { project_path, ctx }
    }

    pub async fn run(&self, script_ctx: &Script, name: &str) -> anyhow::Result<()> {
        match script_ctx {
            Script::Shell {} => ShellExecutor::from(self).execute(name).await?,
        }

        Ok(())
    }
}
