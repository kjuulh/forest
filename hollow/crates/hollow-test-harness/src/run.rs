//! Invoke the runner over SSH, parse its JSONL event stream, return an
//! assertable [`RunResult`].

use std::collections::HashMap;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::bootstrap::RemoteLayout;
use crate::config::Config;
use crate::ssh;

/// Job-level inputs the test author cares about. Mirrors the runner's
/// `JobSpec` but with friendlier file plumbing (raw bytes, not base64).
#[derive(Debug, Clone, Default)]
pub struct JobSpec {
    pub command: Vec<String>,
    pub environment: HashMap<String, String>,
    pub files: Vec<JobFile>,
    /// "deploy" or "plan". Defaults to "deploy".
    pub mode: Option<String>,
    /// vCPUs for the microVM. Defaults to 1.
    pub vcpus: Option<u8>,
    /// Memory (MiB) for the microVM. Defaults to 512.
    pub mem_mib: Option<u32>,
    /// Hard wallclock cap on the runner (seconds). Defaults to 120.
    pub timeout_seconds: Option<u32>,
    /// Enable per-VM NAT networking. The harness picks a unique subnet index
    /// and asks the runner (which detects the host outbound iface) to wire
    /// the tap + iptables. Required for any job that touches the network
    /// (cloud APIs, terraform registry, package downloads, etc.).
    pub network: bool,
}

#[derive(Debug, Clone)]
pub struct JobFile {
    pub path: String,
    pub content: Vec<u8>,
    pub mode: u32,
}

/// Outcome of a single test job. Stages and logs are kept in arrival order.
#[derive(Debug)]
pub struct RunResult {
    pub exit_code: i32,
    pub plan_output: Option<String>,
    pub stages: Vec<String>,
    pub logs: Vec<LogLine>,
    pub diagnostics: Vec<Diagnostic>,
    pub raw_events: Vec<Event>,
}

impl RunResult {
    pub fn log_lines(&self) -> impl Iterator<Item = &str> {
        self.logs.iter().map(|l| l.line.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub channel: String,
    pub line: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: String,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Stage {
        stage: String,
    },
    Diag {
        level: String,
        message: String,
    },
    Log {
        channel: String,
        line: String,
        timestamp: u64,
    },
    Completion {
        exit_code: i32,
        plan_output: Option<String>,
    },
    RunnerError {
        message: String,
    },
}

#[derive(Serialize)]
struct WireRunnerSpec<'a> {
    firecracker_bin: &'a str,
    kernel: &'a str,
    rootfs: &'a str,
    job: WireJobSpec,
    vcpus: u8,
    mem_mib: u32,
    timeout_seconds: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    network: Option<WireNetworkSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    jailer: Option<WireJailerSpec>,
}

#[derive(Serialize)]
struct WireNetworkSpec {
    subnet_index: u8,
    host_iface: String,
    dns: Vec<String>,
}

#[derive(Serialize)]
struct WireJailerSpec {
    jailer_bin: String,
    chroot_base: String,
    uid: u32,
    gid: u32,
}

#[derive(Serialize)]
struct WireJobSpec {
    command: Vec<String>,
    environment: HashMap<String, String>,
    files: Vec<WireJobFile>,
    mode: String,
}

#[derive(Serialize)]
struct WireJobFile {
    path: String,
    content_b64: String,
    mode: u32,
}

pub fn execute(cfg: &Config, layout: &RemoteLayout, job: JobSpec) -> anyhow::Result<RunResult> {
    let network = if job.network {
        // Each direct-runner test uses subnet 0 — single VM at a time over
        // this invocation. Agent-dispatched tests rely on the agent's own
        // allocator to pick a unique index.
        Some(WireNetworkSpec {
            subnet_index: 0,
            host_iface: layout.host_iface.clone(),
            dns: vec!["1.1.1.1".into(), "8.8.8.8".into()],
        })
    } else {
        None
    };
    // Jailer is always-on for the runner path. Production agents do the same;
    // there's no scenario where we want unjailed Firecracker in tests.
    let jailer = Some(WireJailerSpec {
        jailer_bin: layout.jailer_bin.clone(),
        chroot_base: layout.jailer_chroot_base.clone(),
        uid: layout.jailer_uid,
        gid: layout.jailer_gid,
    });
    let wire = WireRunnerSpec {
        firecracker_bin: &layout.firecracker_bin,
        kernel: &layout.kernel,
        rootfs: &layout.rootfs,
        job: WireJobSpec {
            command: job.command,
            environment: job.environment,
            files: job
                .files
                .into_iter()
                .map(|f| {
                    use base64::Engine;
                    WireJobFile {
                        path: f.path,
                        content_b64: base64::engine::general_purpose::STANDARD.encode(&f.content),
                        mode: f.mode,
                    }
                })
                .collect(),
            mode: job.mode.unwrap_or_else(|| "deploy".to_string()),
        },
        vcpus: job.vcpus.unwrap_or(1),
        mem_mib: job.mem_mib.unwrap_or(512),
        timeout_seconds: job.timeout_seconds.unwrap_or(120),
        network,
        jailer,
    };
    let payload = serde_json::to_vec(&wire)?;

    let cmd = format!(
        "{runner} --workdir-root {workdir}",
        runner = layout.runner_bin,
        workdir = layout.workdir_root,
    );
    let out = ssh::run_remote_streaming(cfg, &cmd, &payload, |line| {
        tee_event(line);
    })
    .context("runner invocation")?;

    if !out.success && out.stdout.is_empty() {
        bail!(
            "runner failed with no events. stderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    parse_events(&out.stdout, &out.stderr)
}

fn parse_events(stdout: &[u8], stderr: &[u8]) -> anyhow::Result<RunResult> {
    let mut stages = Vec::new();
    let mut logs = Vec::new();
    let mut diagnostics = Vec::new();
    let mut raw_events = Vec::new();
    let mut completion: Option<(i32, Option<String>)> = None;
    let mut runner_error: Option<String> = None;

    for (lineno, line) in std::str::from_utf8(stdout)
        .unwrap_or_default()
        .lines()
        .enumerate()
    {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let event: Event = serde_json::from_str(line).with_context(|| {
            format!("parse event line {} ({}…)", lineno + 1, preview(line, 80))
        })?;
        match &event {
            Event::Stage { stage } => stages.push(stage.clone()),
            Event::Diag { level, message } => diagnostics.push(Diagnostic {
                level: level.clone(),
                message: message.clone(),
            }),
            Event::Log {
                channel,
                line,
                timestamp,
            } => logs.push(LogLine {
                channel: channel.clone(),
                line: line.clone(),
                timestamp: *timestamp,
            }),
            Event::Completion {
                exit_code,
                plan_output,
            } => completion = Some((*exit_code, plan_output.clone())),
            Event::RunnerError { message } => runner_error = Some(message.clone()),
        }
        raw_events.push(event);
    }

    if let Some(err) = runner_error {
        bail!(
            "runner error: {err}\nremote stderr:\n{}",
            String::from_utf8_lossy(stderr)
        );
    }
    let (exit_code, plan_output) = completion.ok_or_else(|| {
        anyhow::anyhow!(
            "runner exited without a completion event. stages: {stages:?}\nremote stderr:\n{}",
            String::from_utf8_lossy(stderr)
        )
    })?;

    Ok(RunResult {
        exit_code,
        plan_output,
        stages,
        logs,
        diagnostics,
        raw_events,
    })
}

fn preview(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Pretty-print one event as it streams back from the runner so the test
/// session shows live progress (especially valuable for the guest console and
/// job log lines). Falls back to printing the raw line if parsing fails.
fn tee_event(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    match serde_json::from_str::<Event>(line) {
        Ok(Event::Stage { stage }) => eprintln!("[vm]      stage: {stage}"),
        Ok(Event::Diag { level, message }) => eprintln!("[runner]  {level}: {message}"),
        Ok(Event::Log {
            channel, line: l, ..
        }) => eprintln!("[{channel}]  {l}"),
        Ok(Event::Completion {
            exit_code,
            plan_output,
        }) => {
            eprintln!("[vm]      completion: exit_code={exit_code}");
            if let Some(p) = plan_output {
                eprintln!("[vm]      plan_output: {} bytes", p.len());
            }
        }
        Ok(Event::RunnerError { message }) => eprintln!("[runner]  ERROR: {message}"),
        Err(_) => eprintln!("[runner-raw] {line}"),
    }
}
