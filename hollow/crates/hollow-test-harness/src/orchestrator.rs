//! Full controller+agent orchestration for acceptance tests.
//!
//! Starts, in order:
//!   1. a [`FakeServer`](crate::fake_server::FakeServer) (puppet forest-server),
//!   2. `hollow-controller` as a subprocess on the dev machine, pointed at the
//!      fake server,
//!   3. `hollow-agent` as a child process running on the remote KVM host via
//!      SSH, with a reverse port forward (`ssh -R`) so the agent can reach the
//!      dev-machine controller without requiring inbound network access.
//!
//! All processes inherit `kill_on_drop` / `-tt` semantics so dropping the
//! [`Orchestrator`] tears everything down.

use std::net::TcpListener;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use tokio::process::Child;

use crate::bootstrap::RemoteLayout;
use crate::config::Config;
use crate::fake_server::FakeServer;

pub struct Orchestrator {
    pub fake_server: FakeServer,
    pub controller_listen_port: u16,
    pub remote_layout: RemoteLayout,
    /// hollow-controller subprocess (killed on drop).
    controller: Child,
    /// SSH session that owns the reverse tunnel AND the remote agent process.
    /// Killing this ssh process closes the tunnel and SIGHUPs the remote shell,
    /// which in turn kills hollow-agent.
    agent_ssh: Child,
}

impl Orchestrator {
    /// Boot the full stack. Blocks until the controller is listening, the
    /// fake server has accepted the controller's registration, and the agent
    /// has registered with the controller.
    pub async fn start(
        cfg: &Config,
        layout: RemoteLayout,
        capability_name: &str,
    ) -> anyhow::Result<Self> {
        let fake_server = FakeServer::start().await?;
        tracing::info!(addr = %fake_server.addr, "fake forest-server up");

        let controller_port = pick_ephemeral_port()?;
        let metrics_port = pick_ephemeral_port()?;
        let controller = spawn_controller(
            cfg,
            controller_port,
            metrics_port,
            &fake_server.endpoint(),
            capability_name,
        )?;
        tracing::info!(port = controller_port, "hollow-controller spawned");

        // Wait for the controller to register with the fake server so by the
        // time the agent starts, the controller is ready to accept it.
        fake_server
            .wait_for_runner(Duration::from_secs(15))
            .await
            .context("controller did not register with fake server")?;
        tracing::info!("controller registered with fake server");

        let agent_ssh = spawn_remote_agent(cfg, &layout, controller_port)?;
        tracing::info!("hollow-agent spawned on remote");

        // Wait until the agent has registered with the controller. The
        // controller's AgentPool increments on register; we infer readiness by
        // waiting a bounded time and letting the first job fail fast with a
        // useful message if registration didn't happen.
        tokio::time::sleep(Duration::from_secs(2)).await;

        Ok(Self {
            fake_server,
            controller_listen_port: controller_port,
            remote_layout: layout,
            controller,
            agent_ssh,
        })
    }
}

impl Drop for Orchestrator {
    fn drop(&mut self) {
        let _ = self.controller.start_kill();
        let _ = self.agent_ssh.start_kill();
    }
}

fn spawn_controller(
    cfg: &Config,
    listen_port: u16,
    metrics_port: u16,
    server_addr: &str,
    capability: &str,
) -> anyhow::Result<Child> {
    // Controller binary was built by build::build() → target/release/hollow-controller.
    let controller_bin = cfg.repo_root.join("target/release/hollow-controller");
    let runner_id = format!("hollow-test-{}", uuid::Uuid::new_v4());

    let mut cmd = tokio::process::Command::new(&controller_bin);
    cmd.env("FOREST_SERVER_ADDR", server_addr)
        .env("HOLLOW_LISTEN_ADDR", format!("127.0.0.1:{listen_port}"))
        .env("HOLLOW_METRICS_ADDR", format!("127.0.0.1:{metrics_port}"))
        .env("HOLLOW_RUNNER_ID", runner_id)
        .env("HOLLOW_MAX_CONCURRENT", "1")
        .env("HOLLOW_CAPABILITIES", capability)
        .env("RUST_LOG", "hollow=debug,info")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);

    cmd.spawn()
        .with_context(|| format!("spawn {}", controller_bin.display()))
}

fn spawn_remote_agent(
    cfg: &Config,
    layout: &RemoteLayout,
    controller_port: u16,
) -> anyhow::Result<Child> {
    // Remote command: export env vars, then exec the agent. Using `exec` so
    // the agent replaces the shell and receives SIGHUP directly when ssh dies.
    let remote_cmd = format!(
        r#"export HOLLOW_CONTROLLER_ADDR=http://127.0.0.1:{port}
export HOLLOW_DATA_DIR={data}
export HOLLOW_VCPUS=2
export HOLLOW_MEMORY_MIB=2048
export HOLLOW_FIRECRACKER_BIN={fc}
export HOLLOW_KERNEL={kernel}
export HOLLOW_IMAGES_DIR={images}
export HOLLOW_JAILER_BIN={jailer_bin}
export HOLLOW_JAILER_CHROOT_BASE={jailer_chroot}
export HOLLOW_JAILER_UID={jailer_uid}
export HOLLOW_JAILER_GID={jailer_gid}
export RUST_LOG=hollow=debug,info
exec {agent}"#,
        port = controller_port,
        data = layout.agent_data_dir,
        fc = layout.firecracker_bin,
        kernel = layout.kernel,
        images = layout.images_dir,
        jailer_bin = layout.jailer_bin,
        jailer_chroot = layout.jailer_chroot_base,
        jailer_uid = layout.jailer_uid,
        jailer_gid = layout.jailer_gid,
        agent = layout.agent_bin,
    );

    let mut cmd = tokio::process::Command::new("ssh");
    if let Some(key) = &cfg.ssh_key {
        cmd.arg("-i").arg(key);
    }
    cmd.args([
        "-tt",
        // Reverse port forward: remote port → our local port.
        "-R",
        &format!("{controller_port}:127.0.0.1:{controller_port}"),
        // Keepalives so the agent doesn't sit idle and drop the tunnel.
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ExitOnForwardFailure=yes",
    ])
    .arg(&cfg.host)
    .arg(&remote_cmd)
    .stdin(Stdio::null())
    .stdout(Stdio::inherit())
    .stderr(Stdio::inherit())
    .kill_on_drop(true);

    cmd.spawn().context("spawn ssh agent session")
}

fn pick_ephemeral_port() -> anyhow::Result<u16> {
    // Bind-and-drop trick: the OS returns an unused port, we close the socket
    // immediately, and the caller re-binds. There's a small TOCTOU window, but
    // for sequential test runs it's reliable enough.
    let listener = TcpListener::bind("127.0.0.1:0").context("bind ephemeral port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}
