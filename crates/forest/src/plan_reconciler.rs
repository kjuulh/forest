use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::model::Project;

pub mod local;
pub mod git {
    use std::{
        env::temp_dir,
        path::{Path, PathBuf},
    };

    use super::local;

    pub async fn reconcile(
        url: &str,
        path: &Option<PathBuf>,
        plan_dir: &Path,
    ) -> anyhow::Result<()> {
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
}

#[derive(Default)]
pub struct PlanReconciler {}

impl PlanReconciler {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn reconcile(
        &self,
        project: &Project,
        destination: &Path,
    ) -> anyhow::Result<Option<PathBuf>> {
        tracing::info!("reconciling project");
        if project.plan.is_none() {
            tracing::debug!("no plan, returning");
            return Ok(None);
        }

        // prepare the plan dir
        // TODO: We're always deleting, consider some form of caching
        let plan_dir = destination.join(".forest").join("plan");
        if plan_dir.exists() {
            tokio::fs::remove_dir_all(&plan_dir).await?;
        }
        tokio::fs::create_dir_all(&plan_dir)
            .await
            .context(anyhow::anyhow!(
                "failed to create plan dir: {}",
                plan_dir.display()
            ))?;

        match project.plan.as_ref().unwrap() {
            crate::model::ProjectPlan::Local { path } => {
                let source = &destination.join(path);
                local::reconcile(source, &plan_dir).await?;
            }
            crate::model::ProjectPlan::Git { url, path } => {
                git::reconcile(url, path, &plan_dir).await?;
            }
            crate::model::ProjectPlan::NoPlan => {
                tracing::debug!("no plan, returning");
                return Ok(None);
            }
        }

        tracing::info!("reconciled project");

        Ok(Some(plan_dir.join("forest.kdl")))
    }
}
