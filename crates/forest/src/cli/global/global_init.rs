use std::path::PathBuf;

use anyhow::Context;

use crate::{services::init::InitServiceState, state::State};

#[derive(clap::Parser)]
pub struct GlobalInitCommand {
    #[arg()]
    starter: Option<String>,

    #[arg(long = "destination", alias = "dest", default_value = ".")]
    dest: PathBuf,
}

impl GlobalInitCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        state
            .init_service()
            .init(&self.starter, &self.dest)
            .await
            .context("init")?;

        Ok(())
    }
}
