use forest_models::Destination;

use crate::{
    destinations::{DestinationEdge, DestinationIndex, logger::DestinationLogger},
    services::release_registry::ReleaseItem,
};

/// Kubernetes is flux2 based currently. (git)
pub struct KubernetesV1Destination {}

#[async_trait::async_trait]
impl DestinationEdge for KubernetesV1Destination {
    fn name(&self) -> DestinationIndex {
        DestinationIndex {
            organisation: "forest".into(),
            name: "kubernetes".into(),
            version: 1,
        }
    }

    async fn release(
        &self,
        _logger: &DestinationLogger,
        _release: &ReleaseItem,
        _destination: &Destination,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}
