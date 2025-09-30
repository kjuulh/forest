use crate::destinations::{DestinationEdge, DestinationIndex};

/// Kubernetes is flux2 based currently. (git)
pub struct KubernetesV1Destination {}

impl DestinationEdge for KubernetesV1Destination {
    fn name(&self) -> DestinationIndex {
        DestinationIndex {
            organisation: "non".into(),
            name: "kubernetes".into(),
            version: 1,
        }
    }
}
