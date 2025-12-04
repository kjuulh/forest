use std::sync::Arc;

use crate::{
    State,
    destinations::{DestinationIndex, DestinationService},
    services::release_logs_registry::ReleaseLogsRegistryState,
};

#[derive(Clone)]
pub struct DestinationServices {
    services: Arc<Vec<DestinationService>>,
}

impl DestinationServices {
    pub fn get_destination(
        &self,
        organisation: &str,
        name: &str,
        version: usize,
    ) -> Option<&DestinationService> {
        let index = DestinationIndex {
            organisation: organisation.into(),
            name: name.into(),
            version,
        };
        self.services.iter().find(|i| i.name() == index)
    }
}

pub trait DestinationServicesState {
    fn destination_services(&self) -> DestinationServices;
}

impl DestinationServicesState for State {
    fn destination_services(&self) -> DestinationServices {
        let release_logs_registry = self.release_logs_registry();
        DestinationServices {
            services: Arc::new(vec![
                DestinationService::new_kubernetes_v1(release_logs_registry.clone()),
                DestinationService::new_terraform_v1(self, release_logs_registry),
            ]),
        }
    }
}
