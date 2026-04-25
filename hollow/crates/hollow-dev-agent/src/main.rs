//! `hollow-dev-agent` — bring up a real `hollow-agent` on the configured
//! remote KVM host for local development.
//!
//! Builds the agent (and dependencies) for the remote target via the harness,
//! rsyncs them, then opens an SSH session with a reverse port forward back to
//! `127.0.0.1:<controller_port>` on the dev machine and execs the agent
//! inside it. The process blocks on the SSH child; Ctrl-C kills SSH which in
//! turn SIGHUPs the remote shell which in turn ends the agent.
//!
//! Configuration via env vars:
//!   HOLLOW_TEST_HOST       — required; SSH alias or user@host
//!   HOLLOW_TEST_KEY        — optional; identity file for ssh
//!   HOLLOW_CONTROLLER_PORT — optional; defaults to 4050 (matches mise
//!                            controller task's HOLLOW_LISTEN_ADDR)
//!   HOLLOW_DEV_CONSOLE     — optional; "true" enables guest console replay
//!                            in the agent. Default off (matches prod).

use anyhow::Context;
use hollow_test_harness::Harness;

const DEFAULT_CONTROLLER_PORT: u16 = 4050;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("hollow=info,info")),
        )
        .init();

    let harness = Harness::from_env().context(
        "HOLLOW_TEST_HOST not set — point it at a KVM-capable host \
         (e.g. forage-local-agent or user@host)",
    )?;

    let controller_port = std::env::var("HOLLOW_CONTROLLER_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_CONTROLLER_PORT);

    let capture_console = std::env::var("HOLLOW_DEV_CONSOLE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let allow_local_egress = std::env::var("HOLLOW_DEV_LOCAL_EGRESS")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    tracing::info!(
        host = %harness.config().host,
        controller_port,
        capture_console,
        allow_local_egress,
        "preparing artefacts and starting remote agent"
    );

    let mut child = harness
        .start_remote_agent(controller_port, capture_console, allow_local_egress)
        .context("start remote agent")?;

    let status = child.wait().await.context("wait for ssh agent session")?;
    tracing::info!(?status, "ssh session ended");
    if !status.success() {
        anyhow::bail!("ssh exited non-zero: {status}");
    }
    Ok(())
}
