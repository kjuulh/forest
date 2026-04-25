//! Shared Firecracker microVM launcher and vsock job-protocol pump.
//!
//! Used by both `hollow-test-runner` (acceptance tests, emits JSONL events on
//! stdout) and `hollow-agent` (production runtime, emits gRPC events to the
//! controller). Consumers wire up their own transport via the `VmEvent`
//! callback passed to [`run_job`].

pub mod firecracker;
pub mod jailer;
pub mod net;
pub mod session;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use hollow_vsock::protocol::{JobDefinition, LogLineMsg};

pub use crate::firecracker::VmInstance;
pub use crate::jailer::{JailerConfig, JailerLimits};
pub use crate::net::{NetworkAllocator, NetworkConfig, NetworkHandle};
pub use crate::session::{GUEST_TO_HOST_PORT, GuestSession, JobEvent, JobOutcome, drive_job};

/// Guest CID — only used by Firecracker internally; the value just needs to be ≥ 3.
pub const DEFAULT_GUEST_CID: u32 = 3;

/// How long to wait for the guest to dial back over vsock once Firecracker
/// reports the VM started. Boot, kernel init, and hollow-guest's vsock connect
/// all happen inside this window.
pub const DEFAULT_GUEST_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Inputs needed to launch one microVM. The `workdir` is created on demand
/// and used to host the Firecracker API socket, vsock UDS, and per-VM logs.
#[derive(Debug, Clone)]
pub struct VmConfig {
    pub firecracker_bin: PathBuf,
    pub kernel: PathBuf,
    pub rootfs: PathBuf,
    pub workdir: PathBuf,
    pub vcpus: u8,
    pub mem_mib: u32,
    /// Override the default kernel cmdline. None → see [`default_boot_args`].
    pub boot_args: Option<String>,
    /// Override the guest CID. None → [`DEFAULT_GUEST_CID`].
    pub guest_cid: Option<u32>,
    /// Override the guest connect timeout. None → [`DEFAULT_GUEST_CONNECT_TIMEOUT`].
    pub guest_connect_timeout: Option<Duration>,
    /// If true, mount rootfs read-only (write-through CoW would go here later).
    pub rootfs_read_only: bool,
    /// If set, the VM gets a tap-backed virtio-net device and NATed egress.
    /// None → no network (vsock only).
    pub network: Option<NetworkConfig>,
    /// If set, Firecracker is launched via `jailer` (chroot + UID drop +
    /// per-VM cgroup) instead of being spawned directly. Production should
    /// always set this; the direct path is for diagnostics.
    pub jailer: Option<JailerConfig>,
}

impl VmConfig {
    pub fn boot_args(&self) -> String {
        if let Some(extra) = self.boot_args.as_deref() {
            return extra.to_string();
        }
        let mut base = default_boot_args();
        if let Some(net) = &self.network {
            base.push(' ');
            base.push_str(&net.kernel_ip_arg());
        }
        base
    }
}

/// Default kernel cmdline for the Firecracker CI test kernels. `init=` runs
/// `hollow-guest` as PID 1, which is what every hollow rootfs image installs
/// at `/usr/local/bin/hollow-guest`.
pub fn default_boot_args() -> String {
    [
        "console=ttyS0",
        "reboot=k",
        "panic=1",
        "pci=off",
        "i8042.noaux",
        "i8042.nomux",
        "i8042.nopnp",
        "i8042.dumbkbd",
        "init=/usr/local/bin/hollow-guest",
    ]
    .join(" ")
}

/// Lifecycle and log events emitted by [`run_job`]. Consumers translate to
/// their preferred transport.
#[derive(Debug, Clone)]
pub enum VmEvent {
    Stage(VmStage),
    /// Diagnostic message from the launcher itself.
    Diag {
        level: &'static str,
        message: String,
    },
    /// Log line from the job process running inside the VM.
    Log(LogLineMsg),
    /// One line of the guest's serial console (kernel dmesg + hollow-guest's
    /// stdout/stderr as PID 1). Emitted post-hoc, after the VM has shut down
    /// and all output is flushed. Useful for diagnosing boot failures, kernel
    /// panics, or vsock handshake issues where no job log was ever produced.
    GuestConsole { line: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmStage {
    VmSpawn,
    VmStart,
    AwaitGuest,
    GuestReady,
    JobDispatched,
    VmShutdown,
}

impl VmStage {
    pub fn name(self) -> &'static str {
        match self {
            Self::VmSpawn => "vm_spawn",
            Self::VmStart => "vm_start",
            Self::AwaitGuest => "await_guest",
            Self::GuestReady => "guest_ready",
            Self::JobDispatched => "job_dispatched",
            Self::VmShutdown => "vm_shutdown",
        }
    }
}

/// High-level driver: launch a microVM, drive the job protocol over vsock,
/// shut down. Always cleans up the VM, even on error paths.
pub async fn run_job<F>(
    config: VmConfig,
    mut job: JobDefinition,
    mut on_event: F,
) -> anyhow::Result<JobOutcome>
where
    F: FnMut(VmEvent),
{
    // Bring up tap + iptables BEFORE spawning Firecracker so the VM's
    // PUT /network-interfaces finds a usable host_dev. The handle stays alive
    // for the whole VM lifetime and auto-teardown happens in Drop.
    let _network = if let Some(net_cfg) = &config.network {
        on_event(VmEvent::Diag {
            level: "info",
            message: format!(
                "network: tap={} subnet={} host={} guest={}",
                net_cfg.tap_name(),
                net_cfg.subnet_cidr(),
                net_cfg.host_ip(),
                net_cfg.guest_ip(),
            ),
        });
        let handle = NetworkHandle::establish(net_cfg.clone())
            .context("establish per-VM network")?;
        // Pass DNS to the guest via env so hollow-guest can write resolv.conf.
        if !net_cfg.dns.is_empty() {
            job.environment
                .entry("HOLLOW_DNS".to_string())
                .or_insert_with(|| net_cfg.dns.join(","));
        }
        Some(handle)
    } else {
        None
    };

    on_event(VmEvent::Stage(VmStage::VmSpawn));
    let mut vm = match &config.jailer {
        Some(jailer_cfg) => {
            // Cap the Firecracker process tree to the same compute the VM
            // was promised. Without this the cgroup is uncapped, so a guest
            // that fork-bombs or balloons memory usage past `mem_size_mib`
            // can starve the host.
            let limits = JailerLimits::from_vm_size(config.vcpus, config.mem_mib);
            VmInstance::spawn_jailed(
                jailer_cfg,
                &limits,
                config.workdir.clone(),
                &config.kernel,
                &config.rootfs,
            )
            .await
            .context("spawn jailed firecracker")?
        }
        None => VmInstance::spawn(&config.firecracker_bin, config.workdir.clone())
            .await
            .context("spawn firecracker")?,
    };

    vm.put_machine_config(config.vcpus, config.mem_mib).await?;
    vm.put_boot_source(
        config.kernel.to_string_lossy().as_ref(),
        &config.boot_args(),
    )
    .await?;
    vm.put_root_drive(
        config.rootfs.to_string_lossy().as_ref(),
        config.rootfs_read_only,
    )
    .await?;

    if let Some(net_cfg) = &config.network {
        vm.put_network_interface("eth0", &net_cfg.tap_name(), &net_cfg.guest_mac())
            .await
            .context("put_network_interface")?;
    }

    let guest_cid = config.guest_cid.unwrap_or(DEFAULT_GUEST_CID);
    vm.put_vsock(guest_cid).await?;

    // Bind the host-side listener BEFORE booting — kernel init is fast and the
    // guest will dial back almost immediately after `InstanceStart`.
    let session = GuestSession::bind(&vm.vsock_uds, GUEST_TO_HOST_PORT)?;

    on_event(VmEvent::Stage(VmStage::VmStart));
    if let Err(e) = vm.start().await {
        let _ = vm.shutdown().await;
        return Err(e).context("InstanceStart");
    }

    on_event(VmEvent::Stage(VmStage::AwaitGuest));
    let connect_timeout = config
        .guest_connect_timeout
        .unwrap_or(DEFAULT_GUEST_CONNECT_TIMEOUT);
    let stream = match session.accept(connect_timeout).await {
        Ok(s) => s,
        Err(e) => {
            let _ = vm.shutdown().await;
            return Err(e).context("guest never connected over vsock");
        }
    };

    on_event(VmEvent::Stage(VmStage::GuestReady));

    let outcome = drive_job(stream, job, |evt| match evt {
        JobEvent::JobDispatched => on_event(VmEvent::Stage(VmStage::JobDispatched)),
        JobEvent::Log(l) => on_event(VmEvent::Log(l)),
        JobEvent::Heartbeat => {}
        JobEvent::UnexpectedMessage(t) => on_event(VmEvent::Diag {
            level: "warn",
            message: format!("unexpected guest message: {t:?}"),
        }),
    })
    .await;

    on_event(VmEvent::Stage(VmStage::VmShutdown));
    let _ = vm.shutdown().await;

    // After shutdown, Firecracker's stdout buffer is flushed. Surface every
    // line so consumers can diagnose failures that happened outside the job
    // protocol (kernel panics, missing /init, vsock handshake errors).
    if let Ok(console) = vm.read_console_log().await {
        emit_console_lines(&console, &mut on_event);
    }

    drop(session);

    outcome
}

/// Max guest console bytes we'll replay through the event channel. A typical
/// boot is ~20 KiB; this cap prevents pathological runaway kernel prints from
/// swamping test output or the controller → forest-server log stream.
const MAX_CONSOLE_BYTES: usize = 256 * 1024;

fn emit_console_lines<F: FnMut(VmEvent)>(console: &str, on_event: &mut F) {
    let slice = if console.len() > MAX_CONSOLE_BYTES {
        &console[console.len() - MAX_CONSOLE_BYTES..]
    } else {
        console
    };
    for line in slice.lines() {
        if line.is_empty() {
            continue;
        }
        on_event(VmEvent::GuestConsole {
            line: line.to_string(),
        });
    }
}
