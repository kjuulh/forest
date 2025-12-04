use async_trait::async_trait;

use crate::state::State;

use super::{
    client::{RegistryClient, RegistryClientContract},
    models::RegistryComponents,
};

#[allow(dead_code)]
pub struct NonRegistryClient {
    host: String,
    client: reqwest::Client,
}

#[async_trait]
impl RegistryClientContract for NonRegistryClient {
    #[tracing::instrument(skip(self), level = "trace")]
    async fn get_components(&self) -> anyhow::Result<RegistryComponents> {
        tracing::warn!("TODO!");

        Ok(RegistryComponents::default())
    }
}

pub trait NonRegistryClientState {
    fn forest_registry_client(&self) -> RegistryClient;
}

impl NonRegistryClientState for State {
    fn forest_registry_client(&self) -> RegistryClient {
        RegistryClient::from(NonRegistryClient {
            host: "http://localhost:4040".into(),
            client: reqwest::Client::default(),
        })
    }
}
