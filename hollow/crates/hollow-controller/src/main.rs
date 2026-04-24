//! hollow-controller: orchestrator that registers as a forest-runner and
//! dispatches jobs to hollow-agents running on pool machines.

mod agent_pool;
mod dispatcher;
mod grpc_server;
mod job_tracker;
mod metrics;
mod state;

use clap::Parser;
use forest_runner::client::ForestRunnerClient;

use crate::dispatcher::DispatcherState;
use crate::grpc_server::AgentGrpcServerState;
use crate::metrics::MetricsServer;
use crate::state::State;

#[derive(Parser)]
#[command(
    name = "hollow-controller",
    about = "Hollow controller — isolated execution orchestrator"
)]
struct Cli {
    /// gRPC address of the forest-server
    #[arg(long, env = "FOREST_SERVER_ADDR")]
    server_addr: String,

    /// Address to listen on for agent connections
    #[arg(long, env = "HOLLOW_LISTEN_ADDR", default_value = "[::]:4050")]
    listen_addr: String,

    /// Runner ID to register with forest-server
    #[arg(long, env = "HOLLOW_RUNNER_ID", default_value_t = default_runner_id())]
    runner_id: String,

    /// Max concurrent jobs across all agents
    #[arg(long, env = "HOLLOW_MAX_CONCURRENT", default_value = "1")]
    max_concurrent: i32,

    /// Destination capabilities to register (comma-separated, e.g. "forest/terraform/1")
    #[arg(long, env = "HOLLOW_CAPABILITIES", value_delimiter = ',')]
    capabilities: Vec<String>,

    /// Address for the Prometheus metrics endpoint
    #[arg(long, env = "HOLLOW_METRICS_ADDR", default_value = "[::]:4051")]
    metrics_addr: String,
}

fn default_runner_id() -> String {
    format!("hollow-{}", uuid::Uuid::new_v4())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    let capabilities = parse_capabilities(&cli.capabilities);
    if capabilities.is_empty() {
        anyhow::bail!("no capabilities configured — set HOLLOW_CAPABILITIES");
    }

    tracing::info!(
        runner_id = %cli.runner_id,
        server = %cli.server_addr,
        listen = %cli.listen_addr,
        capabilities = ?cli.capabilities,
        max_concurrent = cli.max_concurrent,
        "starting hollow-controller"
    );

    let state = State::new(cli.server_addr.clone());
    let listen_addr = cli.listen_addr.parse()?;
    let metrics_addr = cli.metrics_addr.parse()?;

    notmad::Mad::builder()
        .add(MetricsServer::new(metrics_addr))
        .add(state.agent_grpc_server(listen_addr))
        .add(state.dispatcher(
            ForestRunnerClient::new(cli.server_addr),
            cli.runner_id,
            capabilities,
            cli.max_concurrent,
        ))
        .run()
        .await?;

    Ok(())
}

fn parse_capabilities(caps: &[String]) -> Vec<forest_grpc_interface::DestinationCapability> {
    caps.iter()
        .filter_map(|s| {
            let parts: Vec<&str> = s.split('/').collect();
            if parts.len() == 3 {
                Some(forest_grpc_interface::DestinationCapability {
                    organisation: parts[0].to_string(),
                    name: parts[1].to_string(),
                    version: parts[2].parse().unwrap_or(1),
                })
            } else {
                tracing::warn!(cap = %s, "invalid capability format, expected org/name/version");
                None
            }
        })
        .collect()
}
