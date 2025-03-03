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
        if self.run_project(script_ctx, name).await? {
            return Ok(());
        }

        if self.run_plan(script_ctx, name).await? {
            return Ok(());
        }

        anyhow::bail!("script was not found for name: {}", name)
    }

    async fn run_project(&self, script_ctx: &Script, name: &str) -> anyhow::Result<bool> {
        match script_ctx {
            Script::Shell {} => {
                if matches!(
                    ShellExecutor::from(self).execute(name).await?,
                    shell::ScriptStatus::Found
                ) {
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }

    async fn run_plan(&self, script_ctx: &Script, name: &str) -> anyhow::Result<bool> {
        match script_ctx {
            Script::Shell {} => {
                if matches!(
                    ShellExecutor::from_plan(self).execute(name).await?,
                    shell::ScriptStatus::Found
                ) {
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }
}
