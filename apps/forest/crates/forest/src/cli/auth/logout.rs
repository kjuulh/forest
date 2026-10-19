use crate::state::State;

#[derive(clap::Parser)]
pub struct LogoutCommand {}

impl LogoutCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        eprintln!("Logged out");

        Ok(())
    }
}
