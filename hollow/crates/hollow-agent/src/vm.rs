//! VM lifecycle for the production hollow-agent.
//!
//! Launches one Firecracker microVM per `RunJob` via the shared `hollow-vm`
//! crate, bridging its [`VmEvent`](hollow_vm::VmEvent) stream into the gRPC
//! `AgentMessage`s the controller expects.

use std::path::{Path, PathBuf};

use hollow_grpc_interface::{
    AgentMessage, JobLogBatch, JobStatus, JobUpdate, LogLine, RunJob, agent_message,
};
use hollow_vm::{
    NetworkAllocator, NetworkConfig, VmConfig, VmEvent, VmStage, run_job as vm_run_job,
};
use hollow_vsock::protocol::{JobDefinition, JobFile};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Host-side paths the agent needs to launch any VM — plumbed in from the
/// CLI so the production deployment and the test harness configure them
/// independently.
#[derive(Debug, Clone)]
pub struct VmPaths {
    pub firecracker_bin: PathBuf,
    pub kernel: PathBuf,
    pub images_dir: PathBuf,
    /// Host outbound interface for NAT MASQUERADE (e.g. "ens18"). Detected
    /// from the default route at agent startup.
    pub host_iface: String,
    /// DNS servers the guest should use. Passed to hollow-guest via
    /// HOLLOW_DNS so it can populate /etc/resolv.conf before the job runs.
    pub dns: Vec<String>,
}

pub async fn run_job(
    job: RunJob,
    outbound_tx: mpsc::UnboundedSender<AgentMessage>,
    data_dir: &str,
    vm_paths: &VmPaths,
    net_allocator: &NetworkAllocator,
    cancel: CancellationToken,
) {
    let job_id = job.job_id.clone();
    send_status(&outbound_tx, &job_id, JobStatus::Booting, None);

    let result = tokio::select! {
        r = run_job_inner(&job, &outbound_tx, data_dir, vm_paths, net_allocator) => r,
        _ = cancel.cancelled() => {
            tracing::info!(job_id = %job_id, "job cancelled");
            Err(anyhow::anyhow!("job cancelled"))
        }
    };

    match result {
        Ok((exit_code, plan_output)) => {
            let status = if exit_code == 0 {
                JobStatus::Completed
            } else {
                JobStatus::Failed
            };
            let _ = outbound_tx.send(AgentMessage {
                message: Some(agent_message::Message::JobUpdate(JobUpdate {
                    job_id,
                    status: status.into(),
                    error_message: if exit_code != 0 {
                        format!("process exited with code {exit_code}")
                    } else {
                        String::new()
                    },
                    plan_output,
                    exit_code,
                })),
            });
        }
        Err(e) => {
            tracing::error!(job_id = %job_id, error = %e, "job failed");
            send_status(
                &outbound_tx,
                &job_id,
                JobStatus::Failed,
                Some(format!("{e:#}")),
            );
        }
    }
}

async fn run_job_inner(
    job: &RunJob,
    outbound_tx: &mpsc::UnboundedSender<AgentMessage>,
    data_dir: &str,
    vm_paths: &VmPaths,
    net_allocator: &NetworkAllocator,
) -> anyhow::Result<(i32, Option<String>)> {
    let rootfs = resolve_image(&vm_paths.images_dir, &job.image)?;

    let workdir = PathBuf::from(data_dir).join(format!("vm-{}", job.job_id));
    tokio::fs::create_dir_all(&workdir).await?;

    let network = if job.egress_enabled {
        let subnet = net_allocator
            .allocate()
            .map_err(|e| anyhow::anyhow!("allocate subnet: {e}"))?;
        // The AllocatedSubnet is kept alive for the whole VM run via the
        // VmConfig → NetworkHandle chain inside hollow_vm::run_job. We
        // release the slot on the way out below.
        let cfg = NetworkConfig {
            subnet_index: subnet.index,
            host_iface: vm_paths.host_iface.clone(),
            dns: vm_paths.dns.clone(),
        };
        Some((cfg, subnet))
    } else {
        None
    };

    let vm_config = VmConfig {
        firecracker_bin: vm_paths.firecracker_bin.clone(),
        kernel: vm_paths.kernel.clone(),
        rootfs,
        workdir,
        vcpus: job.vcpus.max(1) as u8,
        mem_mib: if job.memory_mib > 0 {
            job.memory_mib
        } else {
            512
        },
        boot_args: None,
        guest_cid: None,
        guest_connect_timeout: None,
        rootfs_read_only: false,
        network: network.as_ref().map(|(c, _)| c.clone()),
    };

    let job_def = JobDefinition {
        job_id: job.job_id.clone(),
        command: job.command.clone(),
        environment: job.environment.clone(),
        files: job
            .files
            .iter()
            .map(|f| JobFile {
                path: f.path.clone(),
                content: f.content.clone(),
                mode: f.mode,
            })
            .collect(),
        mode: if job.mode.is_empty() {
            "deploy".to_string()
        } else {
            job.mode.clone()
        },
        timeout_seconds: job.timeout_seconds,
    };

    let job_id = job.job_id.clone();
    let tx = outbound_tx.clone();
    let running_emitted = std::sync::atomic::AtomicBool::new(false);

    let on_event = |evt: VmEvent| match evt {
        VmEvent::Stage(stage) => {
            tracing::debug!(job_id = %job_id, stage = stage.name(), "vm stage");
            // The controller's state machine wants a Running transition as
            // soon as the guest starts receiving the job — not when the VM
            // process first spawns. Emit it exactly once.
            if stage == VmStage::JobDispatched
                && !running_emitted.swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                send_status(&tx, &job_id, JobStatus::Running, None);
            }
        }
        VmEvent::Diag { level, message } => {
            tracing::info!(job_id = %job_id, level, %message, "vm diag");
        }
        VmEvent::Log(l) => {
            let _ = tx.send(AgentMessage {
                message: Some(agent_message::Message::LogBatch(JobLogBatch {
                    job_id: job_id.clone(),
                    lines: vec![LogLine {
                        channel: l.channel,
                        line: l.line,
                        timestamp: l.timestamp,
                    }],
                })),
            });
        }
        VmEvent::GuestConsole { line } => {
            let _ = tx.send(AgentMessage {
                message: Some(agent_message::Message::LogBatch(JobLogBatch {
                    job_id: job_id.clone(),
                    lines: vec![LogLine {
                        channel: "console".to_string(),
                        line,
                        timestamp: now_millis(),
                    }],
                })),
            });
        }
    };

    let outcome = vm_run_job(vm_config, job_def, on_event).await;
    // Drop the subnet allocation now that the VM is gone. Keep `network` alive
    // until here so Drop runs in order: NetworkHandle (inside run_job) →
    // AllocatedSubnet (here) → allocator gets its slot back.
    drop(network);
    let outcome = outcome?;
    Ok((outcome.exit_code, outcome.plan_output))
}

/// Map a `RunJob.image` label (e.g. `"base"`, `"terraform-v1"`) to the rootfs
/// `.ext4` on disk. We keep the file naming simple for Stage A; image semver
/// resolution is a follow-on milestone.
fn resolve_image(images_dir: &Path, image: &str) -> anyhow::Result<PathBuf> {
    let mut candidates = vec![images_dir.join(format!("{image}.ext4"))];
    // Also allow a bare `base` to resolve to `base.ext4`.
    if !image.contains('.') {
        candidates.push(images_dir.join(format!("{image}.ext4")));
    }
    for c in &candidates {
        if c.exists() {
            return Ok(c.clone());
        }
    }
    anyhow::bail!(
        "no rootfs image found for `{image}` in {} (tried: {})",
        images_dir.display(),
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn send_status(
    tx: &mpsc::UnboundedSender<AgentMessage>,
    job_id: &str,
    status: JobStatus,
    error: Option<String>,
) {
    let _ = tx.send(AgentMessage {
        message: Some(agent_message::Message::JobUpdate(JobUpdate {
            job_id: job_id.to_string(),
            status: status.into(),
            error_message: error.unwrap_or_default(),
            plan_output: None,
            exit_code: 0,
        })),
    });
}
