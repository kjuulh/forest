use std::net::SocketAddr;

use crate::{grpc, state::State};

#[derive(clap::Parser)]
pub struct ServeCommand {
    #[arg(long, env = "NON_HOST", default_value = "127.0.0.1:4040")]
    host: SocketAddr,
}

impl ServeCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        notmad::Mad::builder()
            .add(grpc::GrpcServer {
                host: self.host,
                state: state.clone(),
            })
            .run()
            .await?;

        Ok(())
    }
}
