use crate::state::State;

#[derive(clap::Parser)]
pub struct RunCommand {}

impl RunCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        Ok(())
    }
}
