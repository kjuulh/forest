use std::path::Path;

use anyhow::Context;

use crate::model::Project;

pub mod local;

#[derive(Default)]
pub struct PlanReconciler {}

impl PlanReconciler {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn reconcile(&self, project: &Project, destination: &Path) -> anyhow::Result<()> {
        tracing::info!("reconciling project");
        if project.plan.is_none() {
            tracing::debug!("no plan, returning");
            return Ok(());
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
            crate::model::ProjectPlan::NoPlan => {
                tracing::debug!("no plan, returning")
            }
        }
        Ok(())
    }
}
