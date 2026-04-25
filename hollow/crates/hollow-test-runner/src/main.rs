//! hollow-test-runner: single-shot Firecracker driver, intended to be invoked
//! over SSH by `hollow-test-harness` on a KVM-capable Linux host.
//!
//! Reads a JSON [`RunnerSpec`] (stdin or `--spec-file`), launches one
//! Firecracker microVM with the given kernel + rootfs + vsock via `hollow-vm`,
//! and emits a stream of JSONL [`Event`]s on stdout. Exits with the job's
//! exit code on success, or non-zero on runner-level failure.

mod spec;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use base64::Engine;
use clap::Parser;
use hollow_vm::{VmConfig, VmEvent, run_job};
use hollow_vsock::protocol::{JobDefinition, JobFile, Secret};

use crate::spec::{Event, RunnerSpec};

#[derive(Parser)]
#[command(
    name = "hollow-test-runner",
    about = "Single-shot Firecracker driver for hollow acceptance tests"
)]
struct Cli {
    /// Read the spec from this file instead of stdin.
    #[arg(long)]
    spec_file: Option<PathBuf>,

    /// Where to put the per-VM working directory (api sock, vsock uds, logs).
    #[arg(long, default_value = "/tmp/hollow-test-runner")]
    workdir_root: PathBuf,
}

#[tokio::main]
async fn main() {
    // The runner is invoked over SSH and its stdout is the event channel,
    // so direct any tracing logs (and panics) to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let exit = match run(cli).await {
        Ok(code) => code,
        Err(e) => {
            Event::RunnerError {
                message: format!("{e:#}"),
            }
            .emit();
            1
        }
    };
    std::process::exit(exit);
}

async fn run(cli: Cli) -> anyhow::Result<i32> {
    let spec = read_spec(cli.spec_file.as_deref())?;
    Event::stage("spec_loaded").emit();

    let workdir = cli.workdir_root.join(format!("vm-{}", uuid::Uuid::new_v4()));
    Event::diag("info", format!("workdir: {}", workdir.display())).emit();

    let job_def = build_job_definition(&spec)?;
    let network = spec.network.as_ref().map(|n| hollow_vm::NetworkConfig {
        subnet_index: n.subnet_index,
        host_iface: n.host_iface.clone(),
        dns: n.dns.clone(),
        allow_local_egress: n.allow_local_egress,
        allowed_egress_cidrs: n.allowed_egress_cidrs.clone(),
    });
    let jailer = spec.jailer.as_ref().map(|j| hollow_vm::JailerConfig {
        jailer_bin: PathBuf::from(&j.jailer_bin),
        firecracker_bin: PathBuf::from(&spec.firecracker_bin),
        chroot_base: PathBuf::from(&j.chroot_base),
        uid: j.uid,
        gid: j.gid,
    });
    let vm_config = VmConfig {
        firecracker_bin: PathBuf::from(&spec.firecracker_bin),
        kernel: PathBuf::from(&spec.kernel),
        rootfs: PathBuf::from(&spec.rootfs),
        workdir,
        vcpus: spec.vcpus,
        mem_mib: spec.mem_mib,
        boot_args: None,
        guest_cid: None,
        guest_connect_timeout: None,
        // Match the production agent posture — rootfs is immutable, scratch
        // areas are tmpfs mounted by hollow-guest.
        rootfs_read_only: true,
        network,
        jailer,
        capture_console: spec.capture_console,
    };

    let outer_timeout = Duration::from_secs(spec.timeout_seconds.into());
    let outcome = tokio::time::timeout(
        outer_timeout,
        run_job(vm_config, job_def, emit_vm_event),
    )
    .await
    .map_err(|_| anyhow::anyhow!("runner exceeded timeout of {}s", spec.timeout_seconds))??;

    Event::Completion {
        exit_code: outcome.exit_code,
        plan_output: outcome.plan_output,
    }
    .emit();

    Ok(if outcome.exit_code >= 0 {
        outcome.exit_code
    } else {
        1
    })
}

fn build_job_definition(spec: &RunnerSpec) -> anyhow::Result<JobDefinition> {
    let files = spec
        .job
        .files
        .iter()
        .map(|f| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&f.content_b64)
                .map_err(|e| anyhow::anyhow!("invalid base64 in file {}: {e}", f.path))?;
            Ok::<_, anyhow::Error>(JobFile {
                path: f.path.clone(),
                content: bytes,
                mode: f.mode,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let environment: HashMap<String, String> = spec.job.environment.clone();

    let secrets = spec
        .job
        .secrets
        .iter()
        .map(|s| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&s.content_b64)
                .map_err(|e| anyhow::anyhow!("invalid base64 in secret {}: {e}", s.name))?;
            Ok::<_, anyhow::Error>(Secret {
                name: s.name.clone(),
                target_path: s.target_path.clone(),
                mode: s.mode,
                content: bytes,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(JobDefinition {
        job_id: format!("test-{}", uuid::Uuid::new_v4()),
        command: spec.job.command.clone(),
        environment,
        files,
        mode: spec.job.mode.clone(),
        timeout_seconds: 0, // outer wallclock enforced by run() above
        secrets,
    })
}

/// Bridge VmEvent → JSONL Event on stdout.
fn emit_vm_event(evt: VmEvent) {
    match evt {
        VmEvent::Stage(s) => Event::stage(s.name()).emit(),
        VmEvent::Diag { level, message } => Event::diag(level, message).emit(),
        VmEvent::Log(l) => Event::Log {
            channel: l.channel,
            line: l.line,
            timestamp: l.timestamp,
        }
        .emit(),
        VmEvent::GuestConsole { line } => Event::Log {
            channel: "console".to_string(),
            line,
            timestamp: 0,
        }
        .emit(),
    }
}

fn read_spec(path: Option<&std::path::Path>) -> anyhow::Result<RunnerSpec> {
    let content = match path {
        Some(p) => std::fs::read_to_string(p)
            .with_context(|| format!("read spec file {}", p.display()))?,
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("read spec from stdin")?;
            buf
        }
    };
    serde_json::from_str(&content).context("parse runner spec")
}
