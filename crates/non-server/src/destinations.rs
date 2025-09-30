use std::{fmt::Display, sync::Arc};

use crate::destinations::kubernetesv1::KubernetesV1Destination;

pub mod kubernetesv1;

pub struct DestinationService {
    inner: Arc<dyn DestinationEdge + Send + Sync + 'static>,
}

impl DestinationService {
    pub fn new<T: DestinationEdge + Send + Sync + 'static>(t: T) -> Self {
        Self { inner: Arc::new(t) }
    }

    pub fn new_kubernetes() -> Self {
        Self::new(KubernetesV1Destination {})
    }
}

#[async_trait::async_trait]
pub trait DestinationEdge {
    fn name(&self) -> DestinationIndex;
}

pub struct DestinationIndex {
    pub organisation: String,
    pub name: String,
    pub version: usize,
}

impl Display for DestinationIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "{}/{}.{}",
            self.organisation, self.name, self.version
        ))
    }
}
