use async_trait::async_trait;

use crate::state::State;

use super::{
    client::{RegistryClient, RegistryClientContract},
    models::RegistryComponents,
};

#[allow(dead_code)]
pub struct ForestRegistryClient {
    host: String,
    client: reqwest::Client,
}

#[async_trait]
impl RegistryClientContract for ForestRegistryClient {
    #[tracing::instrument(skip(self), level = "trace")]
    async fn get_components(&self) -> anyhow::Result<RegistryComponents> {
        tracing::warn!("TODO!");

        Ok(RegistryComponents::default())
    }
}

pub trait ForestRegistryClientState {
    fn forest_registry_client(&self) -> RegistryClient;
}

impl ForestRegistryClientState for State {
    fn forest_registry_client(&self) -> RegistryClient {
        RegistryClient::from(ForestRegistryClient {
            host: "http://localhost:4040".into(),
            client: reqwest::Client::default(),
        })
    }
}
