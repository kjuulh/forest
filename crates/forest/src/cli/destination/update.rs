use std::collections::HashMap;

use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct UpdateCommand {
    #[arg(long)]
    name: String,

    #[arg(long = "metadata")]
    metadata: Vec<String>,
}

impl UpdateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
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
            .update_destination(&self.name, metadata)
            .await
            .context("update destination")?;

        Ok(())
    }
}
