use std::net::SocketAddr;

use crate::{
    checks::Checks, destinations::terraformv1::TerraformV1ServerState, grpc,
    release_reaper::ReleaseReaper, runner_manager::RunnerManager, scheduler::SchedulerState,
    servehttp::ServeHttp, state::State,
};

#[derive(clap::Parser)]
pub struct ServeCommand {
    #[arg(long, env = "FOREST_HOST", default_value = "127.0.0.1:4040")]
    host: SocketAddr,

    #[arg(long, env = "FOREST_HTTP_HOST", default_value = "127.0.0.1:4042")]
    http_host: SocketAddr,

    #[arg(
        long,
        env = "FOREST_TERRAFORM_V1_HOST",
        default_value = "127.0.0.1:4041"
    )]
    terraform_host: SocketAddr,

    /// Disable in-process destination execution. Releases will only be
    /// dispatched to remote runners and will fail if no runner is available.
    #[arg(long, env = "FOREST_DISABLE_IN_PROCESS", default_value = "false")]
    disable_in_process: bool,
}

impl ServeCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let runner_manager = RunnerManager::new();

        notmad::Mad::builder()
            .add(grpc::GrpcServer {
                host: self.host,
                state: state.clone(),
                runner_manager: runner_manager.clone(),
            })
            .add(ServeHttp {
                host: self.http_host,
            })
            .add(Checks {
                state: state.clone(),
            })
            .add(state.terraform_v1_server(self.terraform_host))
            .add(state.scheduler(runner_manager.clone(), self.disable_in_process))
            .add(ReleaseReaper::new(state, runner_manager.clone()))
            .add(state.drop_queue.clone())
            .run()
            .await?;

        Ok(())
    }
}
