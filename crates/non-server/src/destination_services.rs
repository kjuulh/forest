use std::{collections::BTreeMap, sync::Arc};

use non_models::Destination;
use tokio::sync::RwLock;

use crate::{State, destinations::DestinationService};

pub struct DestinationServices {
    services: Arc<RwLock<BTreeMap<Destination, DestinationService>>>,
}

impl DestinationServices {}

pub trait DestinationServicesState {
    fn destination_services(&self) -> DestinationServices;
}

impl DestinationServicesState for State {
    fn destination_services(&self) -> DestinationServices {
        DestinationServices {
            services: Arc::default(),
        }
    }
}
