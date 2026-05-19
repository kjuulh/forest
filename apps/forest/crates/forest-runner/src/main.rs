use std::sync::Arc;

use clap::Parser;
use forest_runner::client::ForestRunnerClient;
use forest_runner::destinations::RunnerDestination;
use forest_runner::destinations::fluxv1::FluxV1RunnerDestination;
use forest_runner::executor::Executor;
use forest_runner::service::RunnerService;

#[derive(Parser)]
#[command(name = "forest-runner", about = "Forest runner agent")]
struct Cli {
    /// gRPC address of the forest-server (e.g. http://localhost:5554)
    #[arg(long, env = "FOREST_SERVER_ADDR")]
    server_addr: String,

    /// Unique identifier for this runner instance
    #[arg(long, env = "FOREST_RUNNER_ID", default_value_t = default_runner_id())]
    runner_id: String,

    /// Maximum number of concurrent releases this runner will handle
    #[arg(long, env = "FOREST_MAX_CONCURRENT", default_value = "4")]
    max_concurrent: i32,

    /// Enable all built-in destinations (default if no --destination flags)
    #[arg(long)]
    all: bool,

    /// Destinations to enable (can be repeated or comma-separated): flux
    #[arg(long = "destination", env = "FOREST_DESTINATIONS", value_delimiter = ',')]
    destinations: Vec<String>,
}

fn default_runner_id() -> String {
    format!("runner-{}", uuid::Uuid::new_v4())
}

fn register_destinations(cli: &Cli) -> Vec<Box<dyn RunnerDestination>> {
    let enable_all = cli.all || cli.destinations.is_empty();
    let mut dests: Vec<Box<dyn RunnerDestination>> = Vec::new();

    if enable_all || cli.destinations.iter().any(|d| d == "flux") {
        dests.push(Box::new(FluxV1RunnerDestination));
    }

    // Future: kubernetes, terraform

    dests
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

    let destinations = register_destinations(&cli);
    if destinations.is_empty() {
        anyhow::bail!("no destinations registered");
    }

    // Collect capabilities from all registered destinations
    let capabilities: Vec<_> = destinations
        .iter()
        .flat_map(|d| d.capabilities())
        .collect();

    tracing::info!(
        runner_id = %cli.runner_id,
        server = %cli.server_addr,
        capabilities = ?capabilities.iter().map(|c| format!("{}/{}/{}", c.organisation, c.name, c.version)).collect::<Vec<_>>(),
        max_concurrent = cli.max_concurrent,
        "starting forest-runner"
    );

    let executor = Arc::new(Executor::new(destinations));
    let client = ForestRunnerClient::new(cli.server_addr.clone());

    let runner_service = RunnerService::new(
        client,
        cli.runner_id.clone(),
        capabilities,
        cli.max_concurrent,
        executor,
    );

    notmad::Mad::builder()
        .add(runner_service)
        .run()
        .await?;

    Ok(())
}
