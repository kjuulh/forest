use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::State;

pub struct Checks {
    state: State,
}

impl Component for Checks {
    fn info(&self) -> ComponentInfo {
        "forest-server/checks".into()
    }

    async fn setup(&self) -> Result<(), MadError> {
        let status = nostatus::StatusRegistry::builder().build();
        nostatus::set_global(status);

        Ok(())
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        nostatus::global().run(cancellation_token).await;

        Ok(())
    }
}
