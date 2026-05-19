use std::collections::HashMap;

use anyhow::Context;
use forest_models::Destination;
use forest_runner::destinations::fluxv1::{FluxV1Handler, Mode};
use sqlx::PgPool;

use crate::{
    destinations::{DestinationEdge, DestinationIndex, logger::DestinationLogger},
    services::{
        artifact_staging_registry::ArtifactStagingRegistry, release_registry::ReleaseItem,
    },
    temp_dir::TempDirectories,
};

use super::in_process_backend::InProcessBackend;

/// Flux v2 GitOps destination — thin adapter that delegates to
/// `FluxV1Handler` from the `forest-runner` crate via an `InProcessBackend`.
pub struct FluxV1Destination {
    pub temp: TempDirectories,
    pub artifact_files: ArtifactStagingRegistry,
    pub db: PgPool,
}

impl FluxV1Destination {
    fn create_backend(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> InProcessBackend {
        let identity = forest_runner::backend::ReleaseIdentity {
            release_intent_id: Some(release.release_intent_id.to_string()),
            release_id: Some(release.id.to_string()),
            artifact_id: Some(release.artifact.to_string()),
            organisation: destination.organisation.clone(),
            project: release.project.clone(),
            destination: destination.name.clone(),
            environment: destination.environment.clone(),
        };

        InProcessBackend::new(
            self.artifact_files.clone(),
            self.db.clone(),
            logger.clone(),
            self.temp.clone(),
            release.artifact,
            release.project_id,
            destination.environment.clone(),
        )
        .with_release_identity(identity)
    }
}

#[async_trait::async_trait]
impl DestinationEdge for FluxV1Destination {
    fn name(&self) -> DestinationIndex {
        DestinationIndex {
            organisation: "forest".into(),
            name: "flux".into(),
            version: 1,
        }
    }

    fn description(&self) -> &str {
        "GitOps continuous delivery via Flux v2: commits rendered manifests to a Git repository and reconciles them on-cluster."
    }

    fn metadata_schema(&self) -> Vec<forest_models::MetadataFieldSchema> {
        vec![
            forest_models::MetadataFieldSchema {
                name: "cluster_name".into(),
                label: "Cluster Name".into(),
                description: "Logical name of the target Kubernetes cluster.".into(),
                required: true,
                field_type: "text".into(),
                default_value: String::new(),
            },
            forest_models::MetadataFieldSchema {
                name: "namespace".into(),
                label: "Namespace".into(),
                description: "Kubernetes namespace where resources are deployed.".into(),
                required: true,
                field_type: "text".into(),
                default_value: String::new(),
            },
            forest_models::MetadataFieldSchema {
                name: "git_url".into(),
                label: "Git URL".into(),
                description: "Remote Git repository URL for GitOps sync (mutually exclusive with local_path)."
                    .into(),
                required: false,
                field_type: "url".into(),
                default_value: String::new(),
            },
            forest_models::MetadataFieldSchema {
                name: "git_branch".into(),
                label: "Git Branch".into(),
                description: "Branch to commit rendered manifests to.".into(),
                required: false,
                field_type: "text".into(),
                default_value: "main".into(),
            },
            forest_models::MetadataFieldSchema {
                name: "git_ssh_key_path".into(),
                label: "Git SSH Key Path".into(),
                description: "Path to the SSH private key used for Git authentication.".into(),
                required: false,
                field_type: "text".into(),
                default_value: String::new(),
            },
            forest_models::MetadataFieldSchema {
                name: "git_username".into(),
                label: "Git Username".into(),
                description: "Username for HTTPS Git authentication.".into(),
                required: false,
                field_type: "text".into(),
                default_value: String::new(),
            },
            forest_models::MetadataFieldSchema {
                name: "git_token".into(),
                label: "Git Token".into(),
                description: "Personal access token for HTTPS Git authentication.".into(),
                required: false,
                field_type: "text".into(),
                default_value: String::new(),
            },
            forest_models::MetadataFieldSchema {
                name: "git_author_name".into(),
                label: "Git Author Name".into(),
                description: "Name used for Git commits made by forest.".into(),
                required: false,
                field_type: "text".into(),
                default_value: "forest-release".into(),
            },
            forest_models::MetadataFieldSchema {
                name: "git_author_email".into(),
                label: "Git Author Email".into(),
                description: "Email used for Git commits made by forest.".into(),
                required: false,
                field_type: "text".into(),
                default_value: "forest@release.local".into(),
            },
            forest_models::MetadataFieldSchema {
                name: "local_path".into(),
                label: "Local Path".into(),
                description: "Local filesystem path for the GitOps repository (mutually exclusive with git_url)."
                    .into(),
                required: false,
                field_type: "text".into(),
                default_value: String::new(),
            },
            forest_models::MetadataFieldSchema {
                name: "reconcile_url".into(),
                label: "Reconcile URL".into(),
                description: "Optional Flux Receiver webhook URL to trigger immediate reconciliation after push."
                    .into(),
                required: false,
                field_type: "url".into(),
                default_value: String::new(),
            },
            forest_models::MetadataFieldSchema {
                name: "webhook_secret".into(),
                label: "Webhook Secret".into(),
                description: "Shared HMAC secret for Flux notification webhooks back to forest. When set, Provider/Alert/Secret CRs are auto-generated.".into(),
                required: false,
                field_type: "text".into(),
                default_value: String::new(),
            },
            forest_models::MetadataFieldSchema {
                name: "forest_webhook_url".into(),
                label: "Forest Webhook URL".into(),
                description: "Externally-reachable forest webhook URL for Flux notifications. Required when webhook_secret is set.".into(),
                required: false,
                field_type: "url".into(),
                default_value: String::new(),
            },
            forest_models::MetadataFieldSchema {
                name: "flux_git_repository_name".into(),
                label: "Flux GitRepository Name".into(),
                description: "Name of the Flux GitRepository CR to watch in Alert eventSources.".into(),
                required: false,
                field_type: "text".into(),
                default_value: "flux-system".into(),
            },
        ]
    }

    fn validate_metadata(&self, metadata: &HashMap<String, String>) -> anyhow::Result<()> {
        FluxV1Handler::validate_metadata(metadata)
    }

    async fn prepare(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        let backend = self.create_backend(logger, release, destination);
        let config = InProcessBackend::config_from_destination(destination);
        FluxV1Handler::run(&backend, &config, Mode::Prepare)
            .await
            .context("flux prepare failed")
    }

    async fn release(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        let backend = self.create_backend(logger, release, destination);
        let config = InProcessBackend::config_from_destination(destination);
        FluxV1Handler::run(&backend, &config, Mode::Apply)
            .await
            .context("flux release failed")
    }
}
