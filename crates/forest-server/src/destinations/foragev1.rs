use std::collections::HashMap;

use anyhow::Context;
use forest_grpc_interface::{
    forage_service_client::ForageServiceClient, ApplyResourcesRequest, ContainerServiceSpec,
    ForageResource, RolloutStatus, ScalingPolicy, WatchRolloutRequest,
    forage_resource, Container,
};
use forest_models::Destination;

use crate::{
    destinations::{DestinationEdge, DestinationIndex, logger::DestinationLogger},
    services::release_registry::ReleaseItem,
};

/// Forage containers v1 — sends the desired-state resource bundle to a
/// forage cluster via gRPC (`ForageService.ApplyResources`), then
/// streams `WatchRollout` until the rollout succeeds or fails.
pub struct ForageV1Destination {}

#[async_trait::async_trait]
impl DestinationEdge for ForageV1Destination {
    fn name(&self) -> DestinationIndex {
        DestinationIndex {
            organisation: "forage".into(),
            name: "containers".into(),
            version: 1,
        }
    }

    fn validate_metadata(&self, metadata: &HashMap<String, String>) -> anyhow::Result<()> {
        ForageV1Metadata::validate(metadata)
    }

    async fn prepare(
        &self,
        logger: &DestinationLogger,
        _release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        let meta = ForageV1Metadata::from_metadata(&destination.metadata)?;

        // Validate connectivity to the forage endpoint
        logger.log_stdout(&format!(
            "forage/containers@1: validating connectivity to {} (namespace: {}, region: {})",
            meta.forage_url, meta.namespace, meta.region
        ));

        // Quick connection check — we don't send any payload, just verify the endpoint is reachable
        let _client = ForageServiceClient::connect(meta.forage_url.clone())
            .await
            .context(format!(
                "failed to connect to forage at {}",
                meta.forage_url
            ))?;

        logger.log_stdout("forage/containers@1: connectivity OK");
        Ok(())
    }

    async fn release(
        &self,
        logger: &DestinationLogger,
        release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        let meta = ForageV1Metadata::from_metadata(&destination.metadata)?;

        logger.log_stdout(&format!(
            "forage/containers@1: deploying to {} (namespace: {}, region: {})",
            meta.forage_url, meta.namespace, meta.region
        ));

        // Connect to forage gRPC
        let mut client = ForageServiceClient::connect(meta.forage_url.clone())
            .await
            .context(format!(
                "failed to connect to forage at {}",
                meta.forage_url
            ))?;

        // Build the resource list.  For now we create a single ContainerService
        // resource from the release metadata.  In the future this will parse the
        // actual deployment files from the artifact.
        let resource_name = destination.name.clone();
        let image = meta
            .image
            .unwrap_or_else(|| format!("registry.forage.sh/{}/{}", destination.organisation, resource_name));

        let resource = ForageResource {
            name: resource_name.clone(),
            spec: Some(forage_resource::Spec::ContainerService(
                ContainerServiceSpec {
                    scaling: Some(ScalingPolicy {
                        replicas: meta.replicas,
                        autoscaling: None,
                    }),
                    container: Some(Container {
                        name: resource_name.clone(),
                        image: image.clone(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
        };

        let mut labels = HashMap::new();
        labels.insert("organisation".into(), destination.organisation.clone());
        labels.insert("project".into(), release.project.clone());
        labels.insert("destination".into(), destination.name.clone());
        labels.insert("environment".into(), destination.environment.clone());
        labels.insert("region".into(), meta.region.clone());
        labels.insert("release_id".into(), release.id.to_string());

        logger.log_stdout(&format!(
            "forage/containers@1: applying 1 resource ({resource_name}: {image})"
        ));

        // Apply resources
        let apply_resp = client
            .apply_resources(ApplyResourcesRequest {
                apply_id: release.id.to_string(),
                namespace: meta.namespace.clone(),
                resources: vec![resource],
                labels,
            })
            .await
            .context("ApplyResources RPC failed")?;

        let rollout_id = apply_resp.into_inner().rollout_id;
        logger.log_stdout(&format!(
            "forage/containers@1: rollout started (id: {rollout_id})"
        ));

        // Watch rollout until completion
        let mut stream = client
            .watch_rollout(WatchRolloutRequest {
                rollout_id: rollout_id.clone(),
            })
            .await
            .context("WatchRollout RPC failed")?
            .into_inner();

        let mut final_status = RolloutStatus::Pending;

        while let Some(event) = stream
            .message()
            .await
            .context("WatchRollout stream error")?
        {
            let status = RolloutStatus::try_from(event.status).unwrap_or(RolloutStatus::Unspecified);
            let status_str = status.as_str_name();

            logger.log_stdout(&format!(
                "  {} [{}]: {} — {}",
                event.resource_name, event.resource_kind, status_str, event.message
            ));

            final_status = status;
        }

        match final_status {
            RolloutStatus::Succeeded => {
                logger.log_stdout("forage/containers@1: rollout succeeded");
                Ok(())
            }
            RolloutStatus::Failed => {
                anyhow::bail!("forage/containers@1: rollout failed")
            }
            other => {
                anyhow::bail!(
                    "forage/containers@1: rollout ended with unexpected status: {}",
                    other.as_str_name()
                )
            }
        }
    }
}

/// Validated metadata for the forage/containers@1 destination type.
struct ForageV1Metadata {
    /// gRPC endpoint of the forage cluster (e.g. "http://forage.example.com:4050").
    forage_url: String,

    /// Forage namespace for resource isolation (usually the org name).
    namespace: String,

    /// Region for compute placement.
    region: String,

    /// Container image (optional — defaults to registry.forage.sh/{org}/{dest_name}).
    image: Option<String>,

    /// Replica count (optional — defaults to 1).
    replicas: u32,
}

impl ForageV1Metadata {
    fn validate(metadata: &HashMap<String, String>) -> anyhow::Result<()> {
        if !metadata.contains_key("forage_url") {
            anyhow::bail!(
                "metadata must contain 'forage_url' (gRPC endpoint of the forage cluster)"
            );
        }
        if !metadata.contains_key("namespace") {
            anyhow::bail!(
                "metadata must contain 'namespace' (forage namespace for resource isolation)"
            );
        }
        Ok(())
    }

    fn from_metadata(metadata: &HashMap<String, String>) -> anyhow::Result<Self> {
        Self::validate(metadata)?;
        Ok(Self {
            forage_url: metadata["forage_url"].clone(),
            namespace: metadata["namespace"].clone(),
            region: metadata
                .get("region")
                .cloned()
                .unwrap_or_else(|| "eu-west-1".into()),
            image: metadata.get("image").cloned(),
            replicas: metadata
                .get("replicas")
                .and_then(|r| r.parse().ok())
                .unwrap_or(1),
        })
    }
}
