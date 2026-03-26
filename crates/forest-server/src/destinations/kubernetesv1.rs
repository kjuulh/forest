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

    fn description(&self) -> &str {
        "Deploy to a Kubernetes cluster using Flux v2-based GitOps."
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
        ]
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
