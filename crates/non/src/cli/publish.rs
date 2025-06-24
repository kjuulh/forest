use std::path::PathBuf;

use crate::{services::components::ComponentsServiceState, state::State};

#[derive(clap::Parser)]
pub struct PublishCommand {
    #[arg(long)]
    path: Option<PathBuf>,

    #[arg(long = "quiet", default_value = "false")]
    quiet: bool,
}

impl PublishCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let path = self
            .path
            .clone()
            .ok_or(anyhow::anyhow!("failed to find path"))
            .or_else(|_| std::env::current_dir())?;

        let component_service = state.components_service();

        let component = component_service.get_staging_component(&path).await?;

        let should_upload = if !self.quiet {
            inquire::Confirm::new(&format!(
                "publish: {}/{} @ version: {}",
                component.component_spec.component.namespace,
                component.component_spec.component.name,
                component.component_spec.component.version
            ))
            .with_default(false)
            .prompt()?
        } else {
            // If quiet we always upload
            true
        };

        if !should_upload {
            tracing::warn!(
                "skipping upload of {}",
                component.component_spec.component.name
            );
            return Ok(());
        }

        component_service.deploy_component(component).await?;

        Ok(())
    }
}
