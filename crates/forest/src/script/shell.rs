use std::{path::PathBuf, process::Stdio};

use anyhow::Context;

use super::ScriptExecutor;

pub struct ShellExecutor {
    root: ScriptExecutor,
    ty: ShellType,
}

pub enum ScriptStatus {
    Found,
    NotFound,
}

enum ShellType {
    Plan,
    Project,
}

impl ShellExecutor {
    pub async fn execute(&self, name: &str) -> anyhow::Result<ScriptStatus> {
        let path = &self.get_path();
        let script_path = path.join("scripts").join(format!("{name}.sh"));

        if !script_path.exists() {
            return Ok(ScriptStatus::NotFound);
        }

        let mut cmd = tokio::process::Command::new(&script_path);
        let cmd = cmd.current_dir(&self.root.project_path);
        cmd.stdin(Stdio::inherit());
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());

        let mut proc = cmd.spawn().context(format!(
            "failed to spawn process: {}",
            script_path.display()
        ))?;

        let exit = proc.wait().await?;

        if !exit.success() {
            anyhow::bail!(
                "command: {name} failed with status: {}",
                exit.code().unwrap_or(-1)
            )
        }

        Ok(ScriptStatus::Found)
    }

    fn get_path(&self) -> PathBuf {
        match self.ty {
            //ShellType::Plan => self.root.project_path.join(".forest").join("plan"),
            ShellType::Plan => self.root.project_path.join(".forest").join("plan"),
            ShellType::Project => self.root.project_path.clone(),
        }
    }

    pub fn from_plan(value: &ScriptExecutor) -> Self {
        Self {
            root: value.clone(),
            ty: ShellType::Plan,
        }
    }
}

impl From<&ScriptExecutor> for ShellExecutor {
    fn from(value: &ScriptExecutor) -> Self {
        Self {
            root: value.clone(),
            ty: ShellType::Project,
        }
    }
}
