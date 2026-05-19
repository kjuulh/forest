use crate::{
    models::{Project, Requirements},
    state::State,
};

pub struct RequirementsService {
    // components: ComponentsService,
}

impl RequirementsService {
    pub async fn gather_requirements(&self, _project: Project) -> anyhow::Result<Requirements> {
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
