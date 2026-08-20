use std::collections::HashMap;

use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct UpdateCommand {
    #[arg(long, short = 'o')]
    organisation: String,

    #[arg(long)]
    name: String,

    #[arg(long = "metadata")]
    metadata: Vec<String>,

    /// Replace the destination's sensitive-key set. Repeatable. Omit the flag
    /// entirely to leave the existing set alone; pass `--clear-sensitive` to
    /// empty it.
    #[arg(long = "sensitive", visible_alias = "sensitive-key")]
    sensitive: Vec<String>,

    /// Clear every destination-declared sensitive key. Keys the destination
    /// type declares sensitive stay hidden regardless.
    #[arg(long, conflicts_with = "sensitive")]
    clear_sensitive: bool,
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

        // `None` leaves the stored set untouched, so an update that only
        // touches metadata cannot accidentally unhide a credential.
        let sensitive_keys = if self.clear_sensitive {
            Some(Vec::new())
        } else if self.sensitive.is_empty() {
            None
        } else {
            Some(self.sensitive.clone())
        };

        state
            .grpc_client()
            .update_destination(&self.organisation, &self.name, metadata, sensitive_keys)
            .await
            .context("update destination")?;

        Ok(())
    }
}
