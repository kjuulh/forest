use crate::{services::init::InitServiceState, state::State};

#[derive(clap::Parser)]
pub struct InitCommand {
    #[arg()]
    starter: Option<String>,
}

impl InitCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        state.init_service().init(&self.starter).await?;

        Ok(())
    }
}
