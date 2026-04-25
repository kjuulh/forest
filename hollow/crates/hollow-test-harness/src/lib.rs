//! Dev-machine-side harness used by the hollow-acceptance test crate.
//!
//! Public flow:
//!
//! ```ignore
//! let harness = Harness::from_env().expect("HOLLOW_TEST_HOST not set");
//! harness.prepare()?;          // build artifacts + bootstrap remote (idempotent)
//! let result = harness.run(JobSpec { … })?;
//! assert_eq!(result.exit_code, 0);
//! ```

mod bootstrap;
mod build;
mod config;
pub mod fake_server;
pub mod orchestrator;
mod run;
mod ssh;

use std::sync::OnceLock;

use anyhow::Context;

pub use crate::bootstrap::RemoteLayout;
pub use crate::config::Config;
pub use crate::orchestrator::Orchestrator;
pub use crate::run::{Diagnostic, JobFile, JobSpec, LogLine, RunResult};

pub struct Harness {
    config: Config,
    /// Bootstrapped exactly once per process; cheap to re-run otherwise.
    prepared: OnceLock<RemoteLayout>,
}

impl Harness {
    /// Build a harness from environment variables. Returns `None` when
    /// `HOLLOW_TEST_HOST` is unset so tests can skip cleanly without panicking.
    pub fn from_env() -> Option<Self> {
        Config::from_env().map(|config| Self {
            config,
            prepared: OnceLock::new(),
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Build local artifacts and bootstrap the remote host. Safe to call
    /// repeatedly — subsequent calls are near-instant.
    pub fn prepare(&self) -> anyhow::Result<&RemoteLayout> {
        if let Some(layout) = self.prepared.get() {
            return Ok(layout);
        }
        let artifacts = build::build(&self.config).context("local build")?;
        let layout = bootstrap::bootstrap(&self.config, &artifacts).context("remote bootstrap")?;
        // Race-tolerant: another caller may have already populated.
        let _ = self.prepared.set(layout);
        Ok(self.prepared.get().expect("just set"))
    }

    /// Run a single job inside a fresh microVM and return its outcome.
    pub fn run(&self, job: JobSpec) -> anyhow::Result<RunResult> {
        let layout = self.prepare()?;
        run::execute(&self.config, layout, job)
    }

    /// Boot the full controller + agent stack against a puppet forest-server
    /// for orchestrator-level acceptance tests.
    pub async fn start_orchestrator(
        &self,
        capability_name: &str,
    ) -> anyhow::Result<Orchestrator> {
        let layout = self.prepare()?.clone();
        Orchestrator::start(&self.config, layout, capability_name).await
    }

    /// Build/ship artefacts and launch only the remote agent, wired back to
    /// a controller already running on `127.0.0.1:<controller_port>` via
    /// reverse SSH tunnel. Returns the SSH `Child`; the caller blocks on it
    /// (or kills it for teardown).
    ///
    /// Used by the `hollow-dev-agent` binary that backs the `mise run
    /// dev:agent` task — keeps the deployment recipe in one place instead
    /// of duplicating it in shell.
    pub fn start_remote_agent(
        &self,
        controller_port: u16,
        capture_console: bool,
    ) -> anyhow::Result<tokio::process::Child> {
        let layout = self.prepare()?.clone();
        crate::orchestrator::spawn_remote_agent(
            &self.config,
            &layout,
            controller_port,
            capture_console,
        )
    }
}
