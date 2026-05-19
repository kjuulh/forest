use std::path::PathBuf;

use anyhow::Context;

use crate::{features::init_project::InitProjectState, state::State};

/// Scaffold project files locally (forest.cue + cue.mod) for a new or
/// existing checkout. Filesystem-only — does not register the project on
/// the server. For server registration, use `forest project create`.
#[derive(clap::Parser)]
pub struct InitCommand {
    /// Target directory. Defaults to the current directory.
    #[arg(long = "where", default_value = ".")]
    r#where: PathBuf,

    /// Project name. Used as the `project.name` field in the scaffolded forest.cue.
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
