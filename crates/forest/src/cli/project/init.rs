use std::path::PathBuf;

use anyhow::Context;

use crate::{features::init_project::InitProjectState, state::State};

#[derive(clap::Parser)]
pub struct InitCommand {
    #[arg(long = "where", default_value = ".")]
    r#where: PathBuf,

    #[arg(long)]
    project_name: String,
}

impl InitCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        state
            .init_project()
            .init(&self.r#where, &self.project_name)
            .await
            .context("init")?;

        Ok(())
    }
}
