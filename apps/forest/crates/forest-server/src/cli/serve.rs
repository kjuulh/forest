use std::{net::SocketAddr, time::Duration};

use crate::{
    checks::Checks,
    destinations::terraformv1::TerraformV1ServerState,
    grpc,
    intent_coordinator::IntentCoordinator,
    oauth_reaper::OAuthReaper,
    release_drain::{AfterDrain, ReleaseDrain},
    release_reaper::ReleaseReaper,
    replicas::ReplicaHeartbeat,
    runner_manager::RunnerManager,
    scheduler::SchedulerState,
    servehttp::ServeHttp,
    state::State,
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

    /// Seconds in-flight releases get to finish after SIGTERM before they are
    /// interrupted and failed. The process exits within this plus 25 seconds,
    /// so the orchestrator's stop timeout must be at least that.
    #[arg(long, env = "FOREST_DRAIN_TIMEOUT_SECS", default_value_t = 90)]
    drain_timeout_secs: u64,
}

impl ServeCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let runner_manager = RunnerManager::new();
        let drain = ReleaseDrain::new(Duration::from_secs(self.drain_timeout_secs));

        tracing::info!(
            drain_timeout_secs = self.drain_timeout_secs,
            shutdown_budget_secs = drain.shutdown_budget().as_secs(),
            "graceful drain configured"
        );

        notmad::Mad::builder()
            .add(AfterDrain::new(
                &drain,
                grpc::GrpcServer {
                    host: self.host,
                    state: state.clone(),
                    runner_manager: runner_manager.clone(),
                },
            ))
            .add(AfterDrain::new(
                &drain,
                ServeHttp {
                    host: self.http_host,
                    object_store: state.object_store.clone(),
                    db: state.db.clone(),
                },
            ))
            .add(AfterDrain::new(
                &drain,
                Checks {
                    state: state.clone(),
                    drain: drain.clone(),
                },
            ))
            .add(AfterDrain::new(
                &drain,
                state.terraform_v1_server(self.terraform_host),
            ))
            .add(state.scheduler(
                runner_manager.clone(),
                self.disable_in_process,
                drain.clone(),
            ))
            .add(ReleaseReaper::new(state, runner_manager.clone()))
            .add(OAuthReaper::new(state))
            .add(IntentCoordinator::new(state))
            .add(AfterDrain::new(&drain, state.drop_queue.clone()))
            .add(AfterDrain::new(
                &drain,
                ReplicaHeartbeat::new(state.db.clone(), drain.clone()),
            ))
            .cancellation(Some(drain.shutdown_budget()))
            .run()
            .await?;

        Ok(())
    }
}
