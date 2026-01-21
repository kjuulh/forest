use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct ListCommand {}

impl ListCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let destinations = state
            .grpc_client()
            .get_destinations()
            .await
            .context("update destination")?;

        if destinations.is_empty() {
            println!("No destinations added yet");

            return Ok(());
        }

        eprintln!("destinations\n");

        for destination in destinations {
            println!("{} @ {}", destination.environment, destination.name);
            if destination.metadata.is_empty() {
                continue;
            }

            println!("metadata:");
            for (key, val) in destination.metadata {
                println!("  {key}: {val}")
            }
        }

        Ok(())
    }
}
