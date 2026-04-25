//! Agent service: notmad Component that manages the agent lifecycle.
//! Connect → register → heartbeat → process jobs → reconnect on failure.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use futures::StreamExt;
use hollow_grpc_interface::{
    AgentHeartbeat, AgentMessage, AgentRegister, agent_message, controller_message,
    hollow_agent_service_client::HollowAgentServiceClient,
};
use notmad::{Component, ComponentInfo, MadError};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::vm;

/// Default resource allocation per job, matching dispatcher defaults.
const DEFAULT_VCPUS_PER_JOB: u32 = 1;
const DEFAULT_MEMORY_MIB_PER_JOB: u32 = 1024;

/// Tracks active VMs for heartbeat reporting and cancellation.
struct VmManagerInner {
    active: HashMap<String, CancellationToken>,
}

#[derive(Clone)]
pub struct VmManager {
    inner: Arc<Mutex<VmManagerInner>>,
}

impl VmManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VmManagerInner {
                active: HashMap::new(),
            })),
        }
    }

    /// Register a new job. Returns a cancellation token that, when cancelled,
    /// signals the VM to stop.
    pub fn register_job(&self, job_id: String) -> CancellationToken {
        let token = CancellationToken::new();
        self.inner
            .lock()
            .expect("vm manager lock poisoned")
            .active
            .insert(job_id, token.clone());
        token
    }

    pub fn job_finished(&self, job_id: &str) {
        self.inner
            .lock()
            .expect("vm manager lock poisoned")
            .active
            .remove(job_id);
    }

    pub fn cancel_job(&self, job_id: &str) -> bool {
        let inner = self.inner.lock().expect("vm manager lock poisoned");
        if let Some(token) = inner.active.get(job_id) {
            token.cancel();
            true
        } else {
            false
        }
    }

    pub fn active_count(&self) -> u32 {
        self.inner
            .lock()
            .expect("vm manager lock poisoned")
            .active
            .len() as u32
    }
}

pub struct AgentService {
    controller_addr: String,
    agent_id: String,
    pool: String,
    total_vcpus: u32,
    total_memory_mib: u32,
    data_dir: String,
    /// Cloned into every spawned job so the launcher can find Firecracker, the
    /// kernel, and rootfs images. Owned by `Arc` so we don't pay copy cost
    /// per job.
    vm_paths: Arc<crate::vm::VmPaths>,
    /// Shared across all concurrent VMs on this agent to hand out unique
    /// /30 subnets. See `hollow_vm::net`.
    net_allocator: hollow_vm::NetworkAllocator,
}

impl AgentService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        controller_addr: String,
        agent_id: String,
        pool: String,
        total_vcpus: u32,
        total_memory_mib: u32,
        data_dir: String,
        firecracker_bin: String,
        kernel: String,
        images_dir: String,
        host_iface: String,
        dns: Vec<String>,
        jailer: Option<hollow_vm::JailerConfig>,
        capture_console: bool,
        allow_local_egress: bool,
    ) -> Self {
        // Remove any leftover taps from a previous agent process before we
        // start handing out subnet indexes — otherwise a stale `hlw-5` will
        // make NetworkHandle::establish fail on the next allocation to 5.
        if let Err(e) = hollow_vm::net::clean_stale_taps() {
            tracing::warn!(error = %e, "clean_stale_taps failed (continuing)");
        }

        Self {
            controller_addr,
            agent_id,
            pool,
            total_vcpus,
            total_memory_mib,
            data_dir,
            vm_paths: Arc::new(crate::vm::VmPaths {
                firecracker_bin: firecracker_bin.into(),
                kernel: kernel.into(),
                images_dir: images_dir.into(),
                host_iface,
                dns,
                jailer,
                capture_console,
                allow_local_egress,
            }),
            net_allocator: hollow_vm::NetworkAllocator::new(),
        }
    }

    async fn run_session(&self) -> anyhow::Result<()> {
        tracing::info!("connecting to controller...");

        let channel = tonic::transport::Channel::from_shared(self.controller_addr.clone())
            .context("invalid controller address")?
            .connect()
            .await
            .context("failed to connect to controller")?;

        let mut client = HollowAgentServiceClient::new(channel);
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel::<AgentMessage>();

        let available_images = scan_images(&self.data_dir);
        let disk_mib = detect_disk_mib(&self.data_dir);

        outbound_tx.send(AgentMessage {
            message: Some(agent_message::Message::Register(AgentRegister {
                agent_id: self.agent_id.clone(),
                hostname: hostname(),
                pool: self.pool.clone(),
                total_vcpus: self.total_vcpus,
                total_memory_mib: self.total_memory_mib,
                total_disk_mib: disk_mib,
                kernel_version: kernel_version(),
                firecracker_version: String::new(),
                available_images,
                arch: std::env::consts::ARCH.to_string(),
            })),
        })?;

        let outbound_stream = UnboundedReceiverStream::new(outbound_rx);
        let response = client.register_agent(outbound_stream).await?;
        let mut inbound = response.into_inner();

        tracing::info!("connected and registered");

        let vm_manager = VmManager::new();

        // Heartbeat task
        let heartbeat_tx = outbound_tx.clone();
        let heartbeat_cancel = CancellationToken::new();
        let hb_cancel = heartbeat_cancel.clone();
        let hb_vm_manager = vm_manager.clone();
        let total_vcpus = self.total_vcpus;
        let total_memory_mib = self.total_memory_mib;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = hb_cancel.cancelled() => break,
                    _ = interval.tick() => {
                        let active = hb_vm_manager.active_count();
                        let _ = heartbeat_tx.send(AgentMessage {
                            message: Some(agent_message::Message::Heartbeat(AgentHeartbeat {
                                active_vms: active,
                                available_vcpus: total_vcpus.saturating_sub(active * DEFAULT_VCPUS_PER_JOB),
                                available_memory_mib: total_memory_mib.saturating_sub(active * DEFAULT_MEMORY_MIB_PER_JOB),
                                available_disk_mib: 0,
                                load_1m: load_average(),
                                load_5m: 0.0,
                            })),
                        });
                    }
                }
            }
        });

        let result = async {
            while let Some(msg) = inbound.next().await {
                let msg = msg.context("controller stream error")?;
                match msg.message {
                    Some(controller_message::Message::RegisterAck(ack)) => {
                        if !ack.accepted {
                            anyhow::bail!("registration rejected: {}", ack.reason);
                        }
                        tracing::info!(agent_id = %ack.agent_id, "registration accepted");
                    }
                    Some(controller_message::Message::RunJob(job)) => {
                        tracing::info!(job_id = %job.job_id, image = %job.image, "received job");
                        let tx = outbound_tx.clone();
                        let data_dir = self.data_dir.clone();
                        let vm_paths = self.vm_paths.clone();
                        let net_allocator = self.net_allocator.clone();
                        let mgr = vm_manager.clone();
                        let job_id = job.job_id.clone();
                        let cancel = mgr.register_job(job_id.clone());
                        tokio::spawn(async move {
                            vm::run_job(job, tx, &data_dir, &vm_paths, &net_allocator, cancel).await;
                            mgr.job_finished(&job_id);
                        });
                    }
                    Some(controller_message::Message::CancelJob(cancel_msg)) => {
                        if vm_manager.cancel_job(&cancel_msg.job_id) {
                            tracing::info!(job_id = %cancel_msg.job_id, "VM cancellation signalled");
                        } else {
                            tracing::warn!(job_id = %cancel_msg.job_id, "job not found for cancellation");
                        }
                    }
                    None => {}
                }
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;

        heartbeat_cancel.cancel();
        result
    }
}

impl Component for AgentService {
    fn info(&self) -> ComponentInfo {
        "hollow/agent".into()
    }

    async fn run(&self, cancellation_token: CancellationToken) -> Result<(), MadError> {
        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    tracing::info!("agent shutting down");
                    break;
                }
                result = self.run_session() => {
                    match result {
                        Ok(()) => tracing::info!("session ended cleanly"),
                        Err(e) => tracing::error!(error = %e, "session error"),
                    }
                    tokio::select! {
                        _ = cancellation_token.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    }
                }
            }
        }

        Ok(())
    }
}

/// Scan data_dir for available rootfs images (*.ext4 files).
fn scan_images(data_dir: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return vec![];
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "ext4"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .collect()
}

/// Detect available disk space in MiB for the data directory.
/// Returns 0 if detection fails (non-Unix, invalid path, etc.).
fn detect_disk_mib(data_dir: &str) -> u32 {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        let Ok(path) = CString::new(data_dir) else {
            tracing::warn!(
                path = data_dir,
                "data_dir contains null bytes, skipping disk detection"
            );
            return 0;
        };
        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(path.as_ptr(), &mut stat) == 0 {
                let bytes = stat.f_bavail as u64 * stat.f_frsize as u64;
                return (bytes / (1024 * 1024)) as u32;
            }
        }
    }
    0
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn kernel_version() -> String {
    std::fs::read_to_string("/proc/version")
        .ok()
        .and_then(|v| v.split_whitespace().nth(2).map(|s| s.to_string()))
        .unwrap_or_default()
}

fn load_average() -> f64 {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse().ok()))
        .unwrap_or(0.0)
}
