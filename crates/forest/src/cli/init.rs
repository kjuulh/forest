use std::path::PathBuf;

use crate::{services::init::InitServiceState, state::State};

#[derive(clap::Parser)]
pub struct InitCommand {
    #[arg()]
    starter: Option<String>,

    #[arg(long = "destination", alias = "dest", default_value = ".")]
    dest: PathBuf,
}

impl InitCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        state.init_service().init(&self.starter, &self.dest).await?;

        Ok(())
    }
}
