use crate::state::State;

#[derive(clap::Parser)]
pub struct GlobalAddCommand {}

impl GlobalAddCommand {
    pub async fn execute(self, state: &State) -> anyhow::Result<()> {
        Ok(())
    }
}
