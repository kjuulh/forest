use client::{RegistryClients, RegistryClientsState};

use crate::state::State;

mod client;
mod non_client;

mod models;
use models::*;

pub struct ComponentRegistry {
    clients: RegistryClients,
}

impl ComponentRegistry {
    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn get_components(&self) -> anyhow::Result<RegistryComponents> {
        let components = self.clients.get_components().await?;

        Ok(components)
    }
}

pub trait ComponentRegistryState {
    fn component_registry(&self) -> ComponentRegistry;
}

impl ComponentRegistryState for State {
    fn component_registry(&self) -> ComponentRegistry {
        ComponentRegistry {
            clients: self.registry_clients(),
        }
    }
}
