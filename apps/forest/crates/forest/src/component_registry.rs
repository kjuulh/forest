use crate::{
    component_cache::models::CacheComponents,
    models::Project,
    services::components::{ComponentsService, ComponentsServiceState},
    state::State,
};

pub struct ComponentRegistry {
    components_service: ComponentsService,
}

impl ComponentRegistry {
    pub async fn get_edge_components(&self, project: Option<Project>) -> anyhow::Result<()> {
        self.download_components(project).await?;

        Ok(())
    }

    async fn download_components(
        &self,
        project: Option<Project>,
    ) -> Result<CacheComponents, anyhow::Error> {
        let components = match project {
            Some(p) => self.components_service.get_components_project(p).await?,
            None => self.components_service.get_components_user_config().await?,
        };

        Ok(components.clone())
    }
}

pub trait ComponentRegistryState {
    fn component_registry(&self) -> ComponentRegistry;
}

impl ComponentRegistryState for State {
    fn component_registry(&self) -> ComponentRegistry {
        ComponentRegistry {
            components_service: self.components_service(),
        }
    }
}
