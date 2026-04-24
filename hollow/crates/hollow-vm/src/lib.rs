//! Shared Firecracker microVM launcher and vsock job-protocol pump.
//!
//! Used by both `hollow-test-runner` (acceptance tests, emits JSONL events on
//! stdout) and `hollow-agent` (production runtime, emits gRPC events to the
//! controller). Consumers wire up their own transport via the `VmEvent`
//! callback passed to [`run_job`].

pub mod firecracker;
pub mod session;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use hollow_vsock::protocol::{JobDefinition, LogLineMsg};

pub use crate::firecracker::VmInstance;
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
}

impl VmConfig {
    pub fn boot_args(&self) -> String {
        self.boot_args.clone().unwrap_or_else(default_boot_args)
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
    job: JobDefinition,
    mut on_event: F,
) -> anyhow::Result<JobOutcome>
where
    F: FnMut(VmEvent),
{
    on_event(VmEvent::Stage(VmStage::VmSpawn));
    let mut vm = VmInstance::spawn(&config.firecracker_bin, config.workdir.clone())
        .await
        .context("spawn firecracker")?;

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
