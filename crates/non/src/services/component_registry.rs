use client::{RegistryClients, RegistryClientsState};

use crate::{
    grpc::{GrpcClient, GrpcClientState},
    state::State,
};

mod client;
mod non_client;

pub mod models;
use models::*;

pub struct ComponentRegistry {
    clients: RegistryClients,
    client: GrpcClient,
}

impl ComponentRegistry {
    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn get_components(&self) -> anyhow::Result<RegistryComponents> {
        let components = self.clients.get_components().await?;

        Ok(components)
    }

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn get_component_version(
        &self,
        name: &str,
        namespace: &str,
        version: &str,
    ) -> anyhow::Result<Option<RegistryComponent>> {
        tracing::trace!("get component version");

        let component_version = self
            .client
            .get_component_version(name, namespace, version)
            .await?;

        Ok(component_version.map(|c| RegistryComponent {
            namespace: namespace.into(),
            name: name.into(),
            version: c.version,
            id: c.id,
        }))
    }
}

pub trait ComponentRegistryState {
    fn component_registry(&self) -> ComponentRegistry;
}

impl ComponentRegistryState for State {
    fn component_registry(&self) -> ComponentRegistry {
        ComponentRegistry {
            clients: self.registry_clients(),
            client: self.grpc_client(),
        }
    }
}
