use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

/// Prints the value of a single withheld metadata key.
///
/// One key per invocation on purpose: there is no "show me every secret on this
/// destination" flag, so pulling a credential to a terminal is always a
/// deliberate act against a named key. The server records each reveal.
#[derive(clap::Parser)]
pub struct RevealCommand {
    #[arg(long, short = 'o', visible_alias = "org")]
    organisation: String,

    #[arg(long)]
    name: String,

    /// Metadata key to reveal, as printed by `forest destination list`.
    #[arg(long)]
    key: String,
}

impl RevealCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let value = state
            .grpc_client()
            .reveal_destination_metadata(&self.organisation, &self.name, &self.key)
            .await
            .context("reveal destination metadata")?;

        // Value alone on stdout so `$(...)` capture is clean; the label goes to
        // stderr like the other destination commands.
        eprintln!("{} @ {}", self.name, self.key);
        println!("{value}");

        Ok(())
    }
}
