use std::collections::HashMap;

use anyhow::Context;

use crate::{grpc::GrpcClientState, state::State};

#[derive(clap::Parser)]
pub struct UpdateCommand {
    #[arg(long, short = 'o')]
    organisation: String,

    #[arg(long)]
    name: String,

    /// Set a metadata key, as `key=value`. Repeatable. Keys you do not name are
    /// left alone; pass `--replace-metadata` to make this the whole set instead.
    #[arg(long = "metadata")]
    metadata: Vec<String>,

    /// Treat `--metadata` as the destination's entire metadata, deleting every
    /// key not named. Without this, `--metadata` only adds and overwrites.
    #[arg(long)]
    replace_metadata: bool,

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

        // Replacing is opt-in. The server stores metadata as one document, so a
        // plain `--metadata k=v` used to send a one-entry map and delete
        // everything else — including credentials this client is never shown and
        // so could not have put back.
        if self.replace_metadata && metadata.is_empty() {
            anyhow::bail!(
                "--replace-metadata with no --metadata would delete every key; \
                 pass the keys to keep, or drop the flag"
            );
        }

        state
            .grpc_client()
            .update_destination(
                &self.organisation,
                &self.name,
                metadata,
                sensitive_keys,
                !self.replace_metadata,
            )
            .await
            .context("update destination")?;

        Ok(())
    }
}
