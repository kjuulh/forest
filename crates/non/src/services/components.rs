use crate::state::State;

pub mod models {
    #[derive(Clone, Debug, Default)]
    pub struct Components {}
}

use models::*;

use super::component_registry::{ComponentRegistry, ComponentRegistryState};

pub struct ComponentsService {
    registry: ComponentRegistry,
}

impl ComponentsService {
    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn list_components(&self) -> anyhow::Result<()> {
        tracing::debug!("listing components");

        let components = self.registry.get_components().await?;

        for component in components.items() {
            println!("component: {}", component.fqn())
        }

        Ok(())
    }
}

impl ComponentsService {
    pub async fn get_components(&self) -> anyhow::Result<Components> {
        Ok(Components::default())
    }
}

pub trait ComponentsServiceState {
    fn components_service(&self) -> ComponentsService;
}

impl ComponentsServiceState for State {
    fn components_service(&self) -> ComponentsService {
        ComponentsService {
            registry: self.component_registry(),
        }
    }
}
