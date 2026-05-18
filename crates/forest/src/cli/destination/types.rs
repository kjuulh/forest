use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct TypesCommand {}

impl TypesCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let types = state
            .grpc_client()
            .list_destination_types()
            .await
            .context("list destination types")?;

        if types.is_empty() {
            eprintln!("No destination types available");
            return Ok(());
        }

        eprintln!("Available destination types:\n");

        for t in types {
            println!("  {}/{}@{}", t.organisation, t.name, t.version);
        }

        Ok(())
    }
}
