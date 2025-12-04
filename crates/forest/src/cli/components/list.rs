use crate::{services::components::ComponentsServiceState, state::State};

#[derive(clap::Parser)]
pub struct ListCommand {}

impl ListCommand {
    #[tracing::instrument(skip(self, state), level = "trace")]
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        tracing::debug!("Listing components");

        state.components_service().list_components().await?;

        Ok(())
    }
}
