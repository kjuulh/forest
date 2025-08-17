use crate::{
    component_cache::ComponentCache,
    models::{Project, Requirements},
    services::components::ComponentsService,
    state::State,
};

pub struct RequirementsService {
    // components: ComponentsService,
}

impl RequirementsService {
    pub async fn gather_requirements(&self, project: Project) -> anyhow::Result<Requirements> {
        todo!()
    }
}

pub trait RequirementsServiceState {
    fn requirements_service(&self) -> RequirementsService;
}

impl RequirementsServiceState for State {
    fn requirements_service(&self) -> RequirementsService {
        RequirementsService {}
    }
}
