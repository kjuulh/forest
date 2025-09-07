use crate::{
    component_cache::models::CacheComponents,
    models::Project,
    services::components::{ComponentsService, ComponentsServiceState},
    state::State,
};

// Getting global and remote state has multiple phases.
//
// 1. [x] Gather dependency requirements, which components do we need. This also includes dependency of dependencies. TODO: missing deps of deps
// 2. [x] Download missing dependencies
// 3. [ ] Gather the tree of dependencies, and fulfill requirements
// 4. [ ] Get fulfill edge requirements
// 5. [ ] Get the edge components

pub struct ComponentRegistry {
    components_service: ComponentsService,
}

impl ComponentRegistry {
    pub async fn get_edge_components(&self, project: Option<Project>) -> anyhow::Result<()> {
        let cached_components = self.download_components(project).await?;

        Ok(())
    }

    async fn download_components(
        &self,
        project: Option<Project>,
    ) -> Result<CacheComponents, anyhow::Error> {
        let components = self.components_service.sync_components(project).await?;

        Ok(components)
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
