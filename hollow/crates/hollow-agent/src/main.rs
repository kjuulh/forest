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

    /// Path to the Firecracker binary
    #[arg(long, env = "HOLLOW_FIRECRACKER_BIN")]
    firecracker_bin: String,

    /// Path to the Linux kernel image (uncompressed vmlinux)
    #[arg(long, env = "HOLLOW_KERNEL")]
    kernel: String,

    /// Directory containing rootfs `.ext4` images, named `{image}.ext4`
    #[arg(long, env = "HOLLOW_IMAGES_DIR")]
    images_dir: String,

    /// Host outbound interface used for per-VM NAT MASQUERADE.
    /// Auto-detected from the default route if not set.
    #[arg(long, env = "HOLLOW_HOST_IFACE")]
    host_iface: Option<String>,

    /// Comma-separated DNS servers for the guest's /etc/resolv.conf.
    #[arg(long, env = "HOLLOW_DNS", default_value = "1.1.1.1,8.8.8.8")]
    dns: String,
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

    let host_iface = match cli.host_iface {
        Some(iface) => iface,
        None => hollow_vm::net::detect_host_iface()
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "could not auto-detect outbound iface; falling back to eth0");
                "eth0".to_string()
            }),
    };
    let dns: Vec<String> = cli
        .dns
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    notmad::Mad::builder()
        .add(AgentService::new(
            cli.controller_addr,
            cli.agent_id,
            cli.pool,
            cli.total_vcpus,
            cli.total_memory_mib,
            cli.data_dir,
            cli.firecracker_bin,
            cli.kernel,
            cli.images_dir,
            host_iface,
            dns,
        ))
        .run()
        .await?;

    Ok(())
}
