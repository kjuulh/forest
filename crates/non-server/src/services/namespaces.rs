use crate::state::State;

pub struct NamespaceService {}

impl NamespaceService {
    pub async fn create_namespace(&self, namespace: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

pub trait NamespaceServiceState {
    fn namespace_service(&self) -> NamespaceService;
}

impl NamespaceServiceState for State {
    fn namespace_service(&self) -> NamespaceService {
        NamespaceService {}
    }
}
