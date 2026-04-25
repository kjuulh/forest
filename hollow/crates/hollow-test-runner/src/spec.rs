//! JSON spec consumed by the test runner.
//!
//! The harness writes one of these to stdin (or `--spec-file`) and the runner
//! emits a stream of JSONL events on stdout (see [`Event`]).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct RunnerSpec {
    /// Path to the Firecracker binary on the host.
    pub firecracker_bin: String,
    /// Path to the uncompressed Linux kernel (`vmlinux`).
    pub kernel: String,
    /// Path to the rootfs ext4 image.
    pub rootfs: String,
    /// Job to run inside the VM.
    pub job: JobSpec,
    /// vCPUs allocated to the microVM.
    #[serde(default = "default_vcpus")]
    pub vcpus: u8,
    /// Memory (MiB) allocated to the microVM.
    #[serde(default = "default_mem")]
    pub mem_mib: u32,
    /// Hard wallclock cap on the whole runner (boot + execution + cleanup).
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
    /// Opt-in per-VM networking (tap + iptables NAT). None → vsock-only.
    #[serde(default)]
    pub network: Option<NetworkSpec>,
    /// Optional jailer wrapper. None → spawn Firecracker directly (dev path).
    #[serde(default)]
    pub jailer: Option<JailerSpec>,
    /// Replay the guest serial console as `Log{channel="console"}` events
    /// after the VM exits. Off by default — the console captures kernel
    /// dmesg + anything PID 1 prints, which is a passive secret-leak channel
    /// for production. Tests that assert on boot output set this to true.
    #[serde(default)]
    pub capture_console: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JailerSpec {
    pub jailer_bin: String,
    pub chroot_base: String,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NetworkSpec {
    /// Unique subnet index [0, 255]. Caller is responsible for ensuring it
    /// doesn't collide with other concurrent VMs on the host.
    pub subnet_index: u8,
    /// Host's default-route interface; `hollow_vm::net::detect_host_iface`
    /// is a reasonable way to fill this in.
    pub host_iface: String,
    /// Nameservers written to /etc/resolv.conf inside the guest.
    #[serde(default = "default_dns")]
    pub dns: Vec<String>,
}

fn default_dns() -> Vec<String> {
    vec!["1.1.1.1".into(), "8.8.8.8".into()]
}

fn default_vcpus() -> u8 {
    1
}
fn default_mem() -> u32 {
    512
}
fn default_timeout() -> u32 {
    120
}

#[derive(Debug, Clone, Deserialize)]
pub struct JobSpec {
    pub command: Vec<String>,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    #[serde(default)]
    pub files: Vec<JobFile>,
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_mode() -> String {
    "deploy".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct JobFile {
    pub path: String,
    /// File contents, base64-encoded.
    pub content_b64: String,
    #[serde(default = "default_file_mode")]
    pub mode: u32,
}

fn default_file_mode() -> u32 {
    0o644
}

/// JSONL events emitted on stdout. The harness parses these in real time.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// Lifecycle stages so the harness can show progress.
    Stage { stage: String },
    /// Diagnostic from the runner itself (not the guest).
    Diag { level: String, message: String },
    /// A log line from the job process inside the guest.
    Log {
        channel: String,
        line: String,
        timestamp: u64,
    },
    /// Final outcome.
    Completion {
        exit_code: i32,
        plan_output: Option<String>,
    },
    /// Runner failed to drive the VM (boot failure, vsock failure, etc.).
    /// The job did not run to completion.
    RunnerError { message: String },
}

impl Event {
    pub fn emit(&self) {
        match serde_json::to_string(self) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("event serialization failed: {e}"),
        }
    }

    pub fn stage(name: &str) -> Self {
        Self::Stage {
            stage: name.to_string(),
        }
    }

    pub fn diag(level: &str, message: impl Into<String>) -> Self {
        Self::Diag {
            level: level.to_string(),
            message: message.into(),
        }
    }
}
