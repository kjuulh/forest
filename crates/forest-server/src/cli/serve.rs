use std::net::SocketAddr;

use crate::{
    destinations::terraformv1::TerraformV1ServerState, grpc, scheduler::SchedulerState,
    state::State,
};

#[derive(clap::Parser)]
pub struct ServeCommand {
    #[arg(long, env = "FOREST_HOST", default_value = "127.0.0.1:4040")]
    host: SocketAddr,

    #[arg(
        long,
        env = "FOREST_TERRAFORM_V1_HOST",
        default_value = "127.0.0.1:4041"
    )]
    terraform_host: SocketAddr,
}

impl ServeCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        notmad::Mad::builder()
            .add(grpc::GrpcServer {
                host: self.host,
                state: state.clone(),
            })
            .add(state.terraform_v1_server(self.terraform_host))
            .add(state.scheduler())
            .add(state.drop_queue.clone())
            .run()
            .await?;

        Ok(())
    }
}
