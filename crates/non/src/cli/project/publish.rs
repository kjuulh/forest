use std::collections::BTreeMap;

use crate::{
    services::{components::ComponentsServiceState, project::ProjectParserState},
    state::State,
};

#[derive(clap::Parser)]
pub struct PublishCommand {
    #[arg(long, short = 'm')]
    metadata: Vec<String>,
}

impl PublishCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let mut metadata_values: BTreeMap<String, String> = BTreeMap::new();
        for entry in &self.metadata {
            let (k, v) = entry
                .split_once("=")
                .ok_or(anyhow::anyhow!("entry is not a key value pair: {entry}"))?;

            metadata_values.insert(k.into(), v.into());
        }
        let project = state.project_parser().get_project().await?;

        let component_service = state.components_service();
        component_service.sync_components(Some(project)).await?;

        Ok(())
    }
}
