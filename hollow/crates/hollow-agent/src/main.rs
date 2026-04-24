//! hollow-agent: runs on each pool machine, manages Firecracker microVMs.
//!
//! Connects to the hollow-controller via gRPC, receives RunJob commands,
//! launches VMs, bridges vsock↔gRPC for logs, and reports completion.

mod service;
mod vm;

use clap::Parser;

use crate::service::AgentService;

#[derive(Parser)]
#[command(
    name = "hollow-agent",
    about = "Hollow pool agent — manages Firecracker microVMs"
)]
struct Cli {
    /// gRPC address of the hollow-controller
    #[arg(long, env = "HOLLOW_CONTROLLER_ADDR")]
    controller_addr: String,

    /// Unique agent identifier
    #[arg(long, env = "HOLLOW_AGENT_ID", default_value_t = default_agent_id())]
    agent_id: String,

    /// Pool this agent belongs to
    #[arg(long, env = "HOLLOW_POOL", default_value = "default")]
    pool: String,

    /// Total vCPUs available for VMs
    #[arg(long, env = "HOLLOW_VCPUS", default_value = "2")]
    total_vcpus: u32,

    /// Total memory (MiB) available for VMs
    #[arg(long, env = "HOLLOW_MEMORY_MIB", default_value = "8192")]
    total_memory_mib: u32,

    /// Directory for rootfs images and VM scratch space
    #[arg(long, env = "HOLLOW_DATA_DIR", default_value = "/var/lib/hollow")]
    data_dir: String,
}

fn default_agent_id() -> String {
    format!("agent-{}", uuid::Uuid::new_v4())
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

    tracing::info!(
        agent_id = %cli.agent_id,
        controller = %cli.controller_addr,
        pool = %cli.pool,
        vcpus = cli.total_vcpus,
        memory_mib = cli.total_memory_mib,
        "starting hollow-agent"
    );

    notmad::Mad::builder()
        .add(AgentService::new(
            cli.controller_addr,
            cli.agent_id,
            cli.pool,
            cli.total_vcpus,
            cli.total_memory_mib,
            cli.data_dir,
        ))
        .run()
        .await?;

    Ok(())
}
