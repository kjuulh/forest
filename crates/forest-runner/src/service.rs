use std::sync::Arc;
use std::time::Duration;

use forest_grpc_interface::DestinationCapability;
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

use crate::client::ForestRunnerClient;
use crate::executor::Executor;

/// notmad Component that manages the runner lifecycle:
/// connect → register → process work → reconnect on failure.
pub struct RunnerService {
    client: ForestRunnerClient,
    runner_id: String,
    capabilities: Vec<DestinationCapability>,
    max_concurrent: i32,
    executor: Arc<Executor>,
}

impl RunnerService {
    pub fn new(
        client: ForestRunnerClient,
        runner_id: String,
        capabilities: Vec<DestinationCapability>,
        max_concurrent: i32,
        executor: Arc<Executor>,
    ) -> Self {
        Self {
            client,
            runner_id,
            capabilities,
            max_concurrent,
            executor,
        }
    }

    async fn run_session(&self) -> anyhow::Result<()> {
        tracing::info!("connecting to forest-server...");

        let mut session = self
            .client
            .connect(
                self.runner_id.clone(),
                self.capabilities.clone(),
                self.max_concurrent,
            )
            .await?;

        tracing::info!("connected and registered");

        loop {
            session.send_heartbeat(self.executor.active_count());

            match session.next_work().await {
                Some(assignment) => {
                    tracing::info!(
                        release_token = %assignment.release_token,
                        release_id = %assignment.release_id,
                        "received work assignment"
                    );

                    if let Err(e) = self.executor.execute(&mut session, &assignment).await {
                        tracing::error!(error = %e, "work execution failed");
                    }
                }
                None => {
                    tracing::warn!("server stream closed");
                    return Ok(());
                }
            }
        }
    }
}

impl Component for RunnerService {
    fn info(&self) -> ComponentInfo {
        "forest-runner/runner".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    tracing::info!("runner shutting down");
                    break;
                }
                result = self.run_session() => {
                    match result {
                        Ok(()) => tracing::info!("session ended cleanly"),
                        Err(e) => tracing::error!(error = %e, "session error"),
                    }
                    // Reconnect after delay
                    tokio::select! {
                        _ = cancellation_token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    }
                }
            }
        }

        Ok(())
    }
}
