use std::{ops::Deref, sync::Arc};

#[derive(Clone)]
pub struct SharedState(Arc<State>);

impl SharedState {
    pub async fn new() -> anyhow::Result<Self> {
        Ok(Self(Arc::new(State::new().await?)))
    }
}

impl Deref for SharedState {
    type Target = Arc<State>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct State {}

impl State {
    pub async fn new() -> anyhow::Result<Self> {
        Ok(Self {})
    }
}
