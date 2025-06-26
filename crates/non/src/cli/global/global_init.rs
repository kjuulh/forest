use crate::{services::init::InitServiceState, state::State};

#[derive(clap::Parser)]
pub struct GlobalInitCommand {
    #[arg()]
    starter: Option<String>,
}

impl GlobalInitCommand {
    pub async fn execute(self, state: &State) -> anyhow::Result<()> {
        state.init_service().init(self.starter).await?;

        Ok(())
    }
}
