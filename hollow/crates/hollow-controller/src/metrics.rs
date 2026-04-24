//! Prometheus metrics endpoint for the hollow-controller.

use std::net::SocketAddr;

use metrics_exporter_prometheus::PrometheusBuilder;
use notmad::{Component, ComponentInfo, MadError};
use tokio_util::sync::CancellationToken;

/// Metric names used throughout the controller.
pub mod names {
    pub const JOBS_DISPATCHED: &str = "hollow_jobs_dispatched_total";
    pub const JOBS_COMPLETED: &str = "hollow_jobs_completed_total";
    pub const JOBS_FAILED: &str = "hollow_jobs_failed_total";
    pub const AGENTS_CONNECTED: &str = "hollow_agents_connected";
    pub const JOBS_ACTIVE: &str = "hollow_jobs_active";
}

/// Installs the Prometheus exporter and serves `/metrics` on the given address.
pub struct MetricsServer {
    listen_addr: SocketAddr,
}

impl MetricsServer {
    pub fn new(listen_addr: SocketAddr) -> Self {
        Self { listen_addr }
    }
}

impl Component for MetricsServer {
    fn info(&self) -> ComponentInfo {
        "hollow/metrics".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        tracing::info!(addr = %self.listen_addr, "starting metrics server");

        PrometheusBuilder::new()
            .with_http_listener(self.listen_addr)
            .install()
            .map_err(|e| MadError::Inner(e.into()))?;

        // Describe the metrics
        metrics::describe_counter!(
            names::JOBS_DISPATCHED,
            "Total number of jobs dispatched to agents"
        );
        metrics::describe_counter!(
            names::JOBS_COMPLETED,
            "Total number of jobs completed successfully"
        );
        metrics::describe_counter!(names::JOBS_FAILED, "Total number of jobs that failed");
        metrics::describe_gauge!(
            names::AGENTS_CONNECTED,
            "Number of currently connected agents"
        );
        metrics::describe_gauge!(names::JOBS_ACTIVE, "Number of currently active jobs");

        // Keep alive until shutdown
        cancellation_token.cancelled().await;
        Ok(())
    }
}
