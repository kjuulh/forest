use std::net::SocketAddr;

use crate::state::State;

#[derive(clap::Parser)]
pub struct ServeCommand {
    #[arg(long, env = "NON_HOST", default_value = "http://localhost:4040")]
    host: SocketAddr,
}

impl ServeCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        Ok(())
    }
}
