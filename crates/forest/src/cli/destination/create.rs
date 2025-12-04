use std::collections::HashMap;

use anyhow::Context;
use forest_models::DestinationType;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct CreateCommand {
    #[arg(long)]
    name: String,

    #[arg(long)]
    environment: String,

    #[arg(long = "type")]
    r#type: String,

    #[arg(long = "metadata")]
    metadata: Vec<String>,
}

impl CreateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let (organisation, rest) = self
            .r#type
            .split_once("/")
            .ok_or(anyhow::anyhow!("an organisation and name is required"))?;
        let (name, version) = rest
            .split_once("@")
            .ok_or(anyhow::anyhow!("a name and version is required"))?;

        let version: usize = version
            .parse()
            .context("version is required to be a unsigned integer")?;

        let metadata = self
            .metadata
            .iter()
            .map(|m| {
                m.split_once("=")
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .ok_or(anyhow::anyhow!("metadata requires a 'key=value'"))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;

        state
            .grpc_client()
            .create_destination(
                &self.name,
                &self.environment,
                metadata,
                DestinationType {
                    organisation: organisation.into(),
                    name: name.into(),
                    version,
                },
            )
            .await
            .context("create destination")?;

        Ok(())
    }
}
