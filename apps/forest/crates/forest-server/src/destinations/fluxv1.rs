use std::collections::HashMap;

use anyhow::Context;
use forest_models::Destination;
use forest_runner::destinations::fluxv1::{FluxV1Handler, Mode, ReconcileOutcome};
use sqlx::PgPool;

use crate::{
    destinations::{DestinationEdge, DestinationIndex, logger::DestinationLogger},
    services::{
        artifact_staging_registry::ArtifactStagingRegistry, release_registry::ReleaseItem,
        release_signals,
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
    pub nats: async_nats::Client,
}

/// The release signal a reconcile outcome is recorded as.
pub const RECONCILE_SIGNAL: &str = "reconcile";

/// How a reconcile outcome reads as a release signal: `(status, detail)`, or
/// `None` when nothing was asked of Flux and there is nothing to say.
///
/// A failed reconcile is DEGRADED rather than UNHEALTHY: the release is in
/// git and Flux applies it on its next poll, so nothing is broken, but it is
/// not running yet either, and a gate waiting on `reconcile` HEALTHY must not
/// open on it.
fn reconcile_signal(outcome: &ReconcileOutcome) -> Option<(&'static str, String)> {
    match outcome {
        ReconcileOutcome::NotRequested => None,
        ReconcileOutcome::Triggered { attempts } => Some((
            "HEALTHY",
            format!("Flux receiver accepted the reconcile request (attempt {attempts})"),
        )),
        ReconcileOutcome::Failed { attempts, reason } => Some((
            "DEGRADED",
            format!(
                "reconcile webhook failed after {attempts} attempt(s): {reason}. The release \
                 is pushed; Flux applies it on its next poll"
            ),
        )),
    }
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
                sensitive: false,
            },
            forest_models::MetadataFieldSchema {
                name: "namespace".into(),
                label: "Namespace".into(),
                description: "Kubernetes namespace where resources are deployed.".into(),
                required: true,
                field_type: "text".into(),
                default_value: String::new(),
                sensitive: false,
            },
            forest_models::MetadataFieldSchema {
                name: "git_url".into(),
                label: "Git URL".into(),
                description: "Remote Git repository URL for GitOps sync (mutually exclusive with local_path)."
                    .into(),
                required: false,
                field_type: "url".into(),
                default_value: String::new(),
                sensitive: false,
            },
            forest_models::MetadataFieldSchema {
                name: "git_branch".into(),
                label: "Git Branch".into(),
                description: "Branch to commit rendered manifests to.".into(),
                required: false,
                field_type: "text".into(),
                default_value: "main".into(),
                sensitive: false,
            },
            forest_models::MetadataFieldSchema {
                name: "git_ssh_key_path".into(),
                label: "Git SSH Key Path".into(),
                description: "Path to the SSH private key used for Git authentication.".into(),
                required: false,
                field_type: "text".into(),
                default_value: String::new(),
                // Deliberately not sensitive: this is a filesystem path, not key
                // material. The key itself never enters metadata, and hiding the
                // path only makes a misconfigured runner harder to debug.
                sensitive: false,
            },
            forest_models::MetadataFieldSchema {
                name: "git_username".into(),
                label: "Git Username".into(),
                description: "Username for HTTPS Git authentication.".into(),
                required: false,
                field_type: "text".into(),
                default_value: String::new(),
                sensitive: false,
            },
            forest_models::MetadataFieldSchema {
                name: "git_token".into(),
                label: "Git Token".into(),
                description: "Personal access token for HTTPS Git authentication.".into(),
                required: false,
                field_type: "secret".into(),
                default_value: String::new(),
                sensitive: true,
            },
            forest_models::MetadataFieldSchema {
                name: "git_author_name".into(),
                label: "Git Author Name".into(),
                description: "Name used for Git commits made by forest.".into(),
                required: false,
                field_type: "text".into(),
                default_value: "forest-release".into(),
                sensitive: false,
            },
            forest_models::MetadataFieldSchema {
                name: "git_author_email".into(),
                label: "Git Author Email".into(),
                description: "Email used for Git commits made by forest.".into(),
                required: false,
                field_type: "text".into(),
                default_value: "forest@release.local".into(),
                sensitive: false,
            },
            forest_models::MetadataFieldSchema {
                name: "local_path".into(),
                label: "Local Path".into(),
                description: "Local filesystem path for the GitOps repository (mutually exclusive with git_url)."
                    .into(),
                required: false,
                field_type: "text".into(),
                default_value: String::new(),
                sensitive: false,
            },
            forest_models::MetadataFieldSchema {
                name: "reconcile_url".into(),
                label: "Reconcile URL".into(),
                description: "Optional Flux Receiver webhook URL to trigger immediate reconciliation after push. Contains the Receiver's generated webhook path, which acts as a bearer token."
                    .into(),
                required: false,
                field_type: "url".into(),
                default_value: String::new(),
                // The URL embeds the Receiver's `.status.webhookPath`, and
                // anyone holding it can trigger reconciliation. Cluster-internal
                // in practice, but it is a capability, not configuration.
                sensitive: true,
            },
            forest_models::MetadataFieldSchema {
                name: "webhook_secret".into(),
                label: "Webhook Secret".into(),
                description: "Shared HMAC secret for Flux notification webhooks back to forest. When set, Provider/Alert/Secret CRs are auto-generated.".into(),
                required: false,
                field_type: "secret".into(),
                default_value: String::new(),
                sensitive: true,
            },
            forest_models::MetadataFieldSchema {
                name: "forest_webhook_url".into(),
                label: "Forest Webhook URL".into(),
                description: "Externally-reachable forest webhook URL for Flux notifications. Required when webhook_secret is set.".into(),
                required: false,
                field_type: "url".into(),
                default_value: String::new(),
                sensitive: false,
            },
            forest_models::MetadataFieldSchema {
                name: "flux_git_repository_name".into(),
                label: "Flux GitRepository Name".into(),
                description: "Name of the Flux GitRepository CR to watch in Alert eventSources.".into(),
                required: false,
                field_type: "text".into(),
                default_value: "flux-system".into(),
                sensitive: false,
            },
        ]
    }

    async fn validate_metadata(&self, metadata: &HashMap<String, String>) -> anyhow::Result<()> {
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
            .context("flux prepare failed")?;
        Ok(())
    }

    async fn release(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        let backend = self.create_backend(logger, release, destination);
        let config = InProcessBackend::config_from_destination(destination);
        let outcome = FluxV1Handler::run(&backend, &config, Mode::Apply)
            .await
            .context("flux release failed")?;

        // Recorded here rather than inside the handler, so forest-runner's
        // flux code keeps no dependency on forest-server's storage — the same
        // split as genericv1's provider signals.
        if let Some((status, detail)) = reconcile_signal(&outcome) {
            let row = release_signals::SignalRow {
                name: RECONCILE_SIGNAL.to_string(),
                status: status.to_string(),
                detail,
                destination_name: destination.name.clone(),
                environment: destination.environment.clone(),
                reported_by: "forest/flux@1".to_string(),
                observed_at: chrono::Utc::now(),
            };
            if let Err(e) = release_signals::report(
                &self.db,
                &self.nats,
                release.release_intent_id,
                release.id,
                &destination.organisation,
                &release.project,
                &row,
            )
            .await
            {
                // The deploy is what matters; losing the release over a
                // signal that failed to store would be the wrong trade.
                logger.log_stderr(&format!(
                    "[flux@1] failed to record the {RECONCILE_SIGNAL} signal: {e:#}"
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_recorded_when_flux_was_not_asked() {
        assert_eq!(reconcile_signal(&ReconcileOutcome::NotRequested), None);
    }

    #[test]
    fn an_accepted_reconcile_is_healthy() {
        let (status, _) = reconcile_signal(&ReconcileOutcome::Triggered { attempts: 2 }).unwrap();
        assert_eq!(status, "HEALTHY");
    }

    /// Not UNHEALTHY: the release is in git and lands on Flux's next poll.
    /// Not HEALTHY either, or "SUCCEEDED" goes back to hiding it.
    #[test]
    fn a_failed_reconcile_is_degraded_and_says_why() {
        let (status, detail) = reconcile_signal(&ReconcileOutcome::Failed {
            attempts: 4,
            reason: "could not connect: tcp connect error".into(),
        })
        .unwrap();
        assert_eq!(status, "DEGRADED");
        assert!(detail.contains("4 attempt(s)"), "{detail}");
        assert!(detail.contains("could not connect"), "{detail}");
    }

    #[test]
    fn every_reported_status_is_one_the_signal_store_accepts() {
        for outcome in [
            ReconcileOutcome::Triggered { attempts: 1 },
            ReconcileOutcome::Failed {
                attempts: 1,
                reason: String::new(),
            },
        ] {
            let (status, _) = reconcile_signal(&outcome).unwrap();
            assert!(release_signals::is_valid_status(status), "{status}");
        }
    }
}
