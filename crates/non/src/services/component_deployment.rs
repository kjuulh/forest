use anyhow::Context;

use crate::{
    grpc::{GrpcClient, GrpcClientState},
    state::State,
};

use super::component_parser::models::RawComponent;

pub struct ComponentDeploymentService {
    grpc: GrpcClient,
}

impl ComponentDeploymentService {
    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn deploy_component(&self, raw_component: RawComponent) -> anyhow::Result<()> {
        tracing::debug!(
            component = raw_component.component_spec.component.name,
            version = raw_component.component_spec.component.version,
            "deploying component"
        );

        // Check version difference
        let component = self
            .grpc
            .get_component(
                &raw_component.component_spec.component.name,
                &raw_component.component_spec.component.namespace,
            )
            .await?;

        let current_version =
            semver::Version::parse(&raw_component.component_spec.component.version)
                .context("failed to parse version as semver")?;

        match component {
            Some(component) => {
                tracing::debug!("component already exists");

                let upstream_semver_version = semver::Version::parse(&component.version)
                    .expect("failed to parse semver version");

                if current_version <= upstream_semver_version {
                    tracing::warn!(
                        current_version = current_version.to_string(),
                        upstream_version = upstream_semver_version.to_string(),
                        "semver version was not greater than upstream, skipping"
                    );

                    return Ok(());
                }
            }
            None => {
                tracing::info!("component doesn't exist, uploading");
            }
        }

        // Begin upload
        let upload_context = self
            .grpc
            .begin_upload(
                &raw_component.component_spec.component.name,
                &raw_component.component_spec.component.namespace,
                &current_version.to_string(),
            )
            .await?;

        // Send files
        for path in walkdir::WalkDir::new(&raw_component.path) {
            let path = path?;
            let metadata = path.metadata()?;

            if !metadata.is_file() {
                continue;
            }

            tracing::info!("uploading file: {}", path.path().to_string_lossy());

            let file_content = tokio::fs::read(path.path()).await?;

            let relative_path = path.path().strip_prefix(&raw_component.path)?;
            self.grpc
                .upload_file(&upload_context, relative_path, &file_content)
                .await?;
        }

        self.grpc.commit_upload(&upload_context).await?;

        Ok(())
    }
}

pub trait ComponentDeploymentServiceState {
    fn component_deployment_service(&self) -> ComponentDeploymentService;
}

impl ComponentDeploymentServiceState for State {
    fn component_deployment_service(&self) -> ComponentDeploymentService {
        ComponentDeploymentService {
            grpc: self.grpc_client(),
        }
    }
}
