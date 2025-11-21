use std::sync::Arc;

use crate::{
    State,
    destinations::{DestinationIndex, DestinationService},
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
        DestinationServices {
            services: Arc::new(vec![
                DestinationService::new_kubernetes_v1(self.db.clone()),
                DestinationService::new_terraform_v1(self, self.db.clone()),
            ]),
        }
    }
}
