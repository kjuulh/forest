use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use tokio::sync::OnceCell;

use crate::state::State;

use super::{
    models::{RegistryComponents, RegistryName},
    forest_client::NonRegistryClientState,
};

pub struct RegistryClient {
    pub(crate) inner: Arc<dyn RegistryClientContract + Send + Sync + 'static>,
}

impl RegistryClient {
    pub async fn get_components(&self) -> anyhow::Result<RegistryComponents> {
        self.inner.get_components().await
    }
}

impl<T: RegistryClientContract + Send + Sync + 'static> From<T> for RegistryClient {
    fn from(value: T) -> Self {
        Self {
            inner: Arc::new(value),
        }
    }
}

#[derive(Clone)]
pub struct RegistryClients {
    state: State,

    clients: Arc<OnceCell<BTreeMap<RegistryName, RegistryClient>>>,
}

impl RegistryClients {
    pub async fn get_components(&self) -> anyhow::Result<RegistryComponents> {
        let clients = self.get_clients().await?;

        let mut components = RegistryComponents::default();
        for (client_name, client) in clients {
            tracing::trace!(client_name, "fetching components");

            let reg_components = client.get_components().await?;

            components.merge(reg_components);
        }

        Ok(components)
    }

    // get_clients allows getting access to a list of internal list of clients, which is initialized only once
    async fn get_clients(&self) -> anyhow::Result<&BTreeMap<RegistryName, RegistryClient>> {
        let s = self.state.clone();
        let output = self
            .clients
            .get_or_try_init({
                move || async move {
                    let mut clients = BTreeMap::new();

                    clients.insert("forest".to_string(), s.forest_registry_client());
                    // Get clients from global config

                    Ok::<_, anyhow::Error>(clients)
                }
            })
            .await?;

        Ok(output)
    }
}

pub trait RegistryClientsState {
    fn registry_clients(&self) -> RegistryClients;
}

impl RegistryClientsState for State {
    fn registry_clients(&self) -> RegistryClients {
        RegistryClients {
            state: self.clone(),
            clients: Arc::new(OnceCell::default()),
        }
    }
}

#[async_trait]
pub trait RegistryClientContract {
    async fn get_components(&self) -> anyhow::Result<RegistryComponents>;
}
