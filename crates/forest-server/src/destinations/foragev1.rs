use std::collections::HashMap;

use forest_models::Destination;

use crate::{
    destinations::{DestinationEdge, DestinationIndex, logger::DestinationLogger},
    services::release_registry::ReleaseItem,
};

/// Forage containers v1 — sends the desired-state resource bundle to a
/// forage cluster via gRPC (`ForageService.ApplyResources`), then
/// streams `WatchRollout` until the rollout succeeds or fails.
///
/// The forage server is not implemented yet; this destination validates
/// metadata and prepares the call site so that the actual gRPC dispatch
/// can be wired in once the server exists.
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

    async fn release(
        &self,
        logger: &DestinationLogger,
        _release: &ReleaseItem,
        destination: &Destination,
    ) -> anyhow::Result<()> {
        let meta = ForageV1Metadata::from_metadata(&destination.metadata)?;

        logger.log_stdout(&format!(
            "forage/containers@1: targeting {} (namespace: {})",
            meta.forage_url, meta.namespace
        ));

        // TODO: once the forage server exists, this will:
        // 1. Build ForageResource list from artifact deployment files
        // 2. Call ForageService.ApplyResources
        // 3. Stream WatchRollout to completion
        anyhow::bail!(
            "forage/containers@1 destination is registered but the forage server is not yet available at {}",
            meta.forage_url,
        )
    }
}

/// Validated metadata for the forage/containers@1 destination type.
struct ForageV1Metadata {
    /// gRPC endpoint of the forage cluster (e.g. "https://forage.example.com:443").
    forage_url: String,

    /// Forage namespace for resource isolation.
    namespace: String,
}

impl ForageV1Metadata {
    fn validate(metadata: &HashMap<String, String>) -> anyhow::Result<()> {
        if !metadata.contains_key("forage_url") {
            anyhow::bail!("metadata must contain 'forage_url' (gRPC endpoint of the forage cluster)");
        }
        if !metadata.contains_key("namespace") {
            anyhow::bail!("metadata must contain 'namespace' (forage namespace for resource isolation)");
        }
        Ok(())
    }

    fn from_metadata(metadata: &HashMap<String, String>) -> anyhow::Result<Self> {
        Self::validate(metadata)?;
        Ok(Self {
            forage_url: metadata["forage_url"].clone(),
            namespace: metadata["namespace"].clone(),
        })
    }
}
