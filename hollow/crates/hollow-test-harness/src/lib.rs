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
pub mod fake_registry;
pub mod fake_server;
pub mod orchestrator;
mod run;
mod ssh;

use std::sync::{Mutex, OnceLock};

use anyhow::Context;

pub use crate::bootstrap::RemoteLayout;
pub use crate::config::Config;
pub use crate::orchestrator::Orchestrator;
pub use crate::run::{Diagnostic, JobFile, JobSecret, JobSpec, LogLine, RunResult};

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
    ///
    /// `PREPARE_LOCK` is a process-wide mutex so multiple test threads
    /// (each with their own Harness instance) don't race on docker build
    /// / scp / remote-fs writes inside `build::build` and
    /// `bootstrap::bootstrap`. Without it, the second concurrent caller
    /// surfaces confusing "No such file or directory" errors from cache
    /// stat'ing files the first caller is mid-rename of.
    pub fn prepare(&self) -> anyhow::Result<&RemoteLayout> {
        if let Some(layout) = self.prepared.get() {
            return Ok(layout);
        }
        static PREPARE_LOCK: Mutex<()> = Mutex::new(());
        let _guard = PREPARE_LOCK.lock().expect("prepare lock poisoned");
        // Re-check under the lock — another thread may have prepared while
        // we were waiting.
        if let Some(layout) = self.prepared.get() {
            return Ok(layout);
        }
        let artifacts = build::build(&self.config).context("local build")?;
        let layout = bootstrap::bootstrap(&self.config, &artifacts).context("remote bootstrap")?;
        let _ = self.prepared.set(layout);
        Ok(self.prepared.get().expect("just set"))
    }

    /// Run a single job inside a fresh microVM and return its outcome.
    pub fn run(&self, job: JobSpec) -> anyhow::Result<RunResult> {
        let layout = self.prepare()?;
        run::execute(&self.config, layout, job)
    }

    /// Boot the full controller + agent stack against a puppet forest-server
    /// for orchestrator-level acceptance tests. Defaults to the strict
    /// egress posture (no RFC1918 / dev-machine reachability from the
    /// guest); tests that need to talk to a host-side mock should use
    /// `start_orchestrator_with_egress`.
    pub async fn start_orchestrator(
        &self,
        capability_name: &str,
    ) -> anyhow::Result<Orchestrator> {
        let layout = self.prepare()?.clone();
        Orchestrator::start(&self.config, layout, capability_name).await
    }

    /// Variant that flips per-VM iptables FORWARD to allow the guest to
    /// reach RFC1918 addresses on the dev machine (e.g. an in-process
    /// FakeRegistry on the host).
    pub async fn start_orchestrator_with_egress(
        &self,
        capability_name: &str,
        allow_local_egress: bool,
    ) -> anyhow::Result<Orchestrator> {
        let layout = self.prepare()?.clone();
        Orchestrator::start_with_egress(&self.config, layout, capability_name, allow_local_egress)
            .await
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
        allow_local_egress: bool,
    ) -> anyhow::Result<tokio::process::Child> {
        let layout = self.prepare()?.clone();
        crate::orchestrator::spawn_remote_agent(
            &self.config,
            &layout,
            controller_port,
            capture_console,
            allow_local_egress,
        )
    }
}
