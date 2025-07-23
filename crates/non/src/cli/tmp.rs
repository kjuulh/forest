use anyhow::Context;
use clap::Parser;

use crate::{services::temp_directories::TempDirectoriesState, state::State};

#[derive(Parser)]
pub struct TmpCommand {}

impl TmpCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let random_path = state
            .temp_directories()
            .create_temp()
            .await
            .context("create temp")?;

        println!("{}", random_path.to_string());

        Ok(())
    }
}
