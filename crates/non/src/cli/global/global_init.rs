use crate::state::State;

#[derive(clap::Parser)]
pub struct GlobalInitCommand {}

impl GlobalInitCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        Ok(())
    }
}
