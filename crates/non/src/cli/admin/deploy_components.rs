use std::path::PathBuf;

use crate::{
    services::{
        component_deployment::ComponentDeploymentServiceState,
        component_parser::ComponentParserState,
    },
    state::State,
};

#[derive(clap::Parser, Debug)]
pub struct DeployComponentCommand {
    #[arg()]
    path: String,

    #[arg(long = "all", default_value = "false")]
    all: bool,

    #[arg(long = "quiet", default_value = "false")]
    quiet: bool,
}

impl DeployComponentCommand {
    #[tracing::instrument(skip(state), level = "trace")]
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        tracing::info!("deploying components");

        let mut component_paths = Vec::new();

        let path = PathBuf::from(&self.path);
        component_paths.push(&path);

        let parser = state.component_parser();

        let mut raw_components = Vec::new();
        for component_path in component_paths {
            let raw_component = parser.parse(component_path).await?;
            raw_components.push(raw_component);
        }

        if raw_components.is_empty() {
            anyhow::bail!("no components found");
        }

        let deploy = state.component_deployment_service();
        for raw_component in raw_components {
            let should_upload = if !self.quiet {
                inquire::Confirm::new(&format!(
                    "upload: {} @ version: {}",
                    raw_component.component_spec.component.name,
                    raw_component.component_spec.component.version
                ))
                .with_default(false)
                .prompt()?
            } else {
                // If quiet we always upload
                true
            };

            if should_upload {
                deploy.deploy_component(raw_component).await?;
            } else {
                tracing::warn!(
                    "skipping upload of {}",
                    raw_component.component_spec.component.name,
                );

                continue;
            }
        }

        tracing::info!("deployed components");

        Ok(())
    }
}
