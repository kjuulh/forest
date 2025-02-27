use std::path::PathBuf;

use shell::ShellExecutor;

use crate::model::{Context, Script};

pub mod shell {
    use std::process::Stdio;

    use anyhow::Context;

    use super::ScriptExecutor;

    pub struct ShellExecutor {
        root: ScriptExecutor,
    }

    impl ShellExecutor {
        pub async fn execute(&self, name: &str) -> anyhow::Result<()> {
            let path = &self.root.project_path;
            let script_path = path.join("scripts").join(format!("{name}.sh"));

            if !script_path.exists() {
                anyhow::bail!("script was not found at: {}", script_path.display());
            }

            let mut cmd = tokio::process::Command::new(&script_path);
            let cmd = cmd.current_dir(path);
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

            Ok(())
        }
    }

    impl From<&ScriptExecutor> for ShellExecutor {
        fn from(value: &ScriptExecutor) -> Self {
            Self {
                root: value.clone(),
            }
        }
    }
}

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
