//! `forest tool …` — dev helpers for authoring external tool manifests.
//!
//! Per TASKS/018-global-tools.md §1a.5, only `hash` lives here in this spec.
//! Publishing external manifests is `forest components publish` against a
//! project whose `forest.cue` declares an `external:` block.

use clap::{Parser, Subcommand};

use crate::state::State;

mod hash;

#[derive(Parser)]
pub struct ToolCommand {
    #[command(subcommand)]
    commands: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Download a URL and print sha256s — `archive_sha256` and the
    /// extracted-binary sha256 — for pasting into a `forest.cue` `external:` block.
    Hash(hash::ToolHashCommand),
}

impl ToolCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        match &self.commands {
            Commands::Hash(cmd) => cmd.execute(state).await,
        }
    }
}
