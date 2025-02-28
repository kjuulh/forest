use std::{
    env::temp_dir,
    path::{Path, PathBuf},
};

use super::local;

pub async fn reconcile(url: &str, path: &Option<PathBuf>, plan_dir: &Path) -> anyhow::Result<()> {
    let temp = TempDir::new();
    tokio::fs::create_dir_all(&temp.0).await?;

    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["clone", url, &temp.0.display().to_string(), "--depth=1"]);

    tracing::info!("cloning plan: {}", url);
    let out = cmd.output().await?;
    if !out.status.success() {
        let stdout = std::str::from_utf8(&out.stdout).unwrap_or_default();
        let stderr = std::str::from_utf8(&out.stderr).unwrap_or_default();

        anyhow::bail!("failed to process git plan: {}, {}", stdout, stderr)
    }

    let temp_plan_dir = if let Some(path) = path {
        temp.0.join(path)
    } else {
        temp.0.to_path_buf()
    };

    local::reconcile(&temp_plan_dir, plan_dir).await?;

    drop(temp);

    Ok(())
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        Self(
            temp_dir()
                .join("forest")
                .join(uuid::Uuid::new_v4().to_string()),
        )
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("to be able to remove temp dir");
    }
}
