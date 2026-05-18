//! Host-platform detection (effectful — reads `cfg!`).

use crate::global::manifest::{Arch, Os, PlatformKey};

pub fn current() -> Option<PlatformKey> {
    let os = if cfg!(target_os = "linux") {
        Os::Linux
    } else if cfg!(target_os = "macos") {
        Os::Darwin
    } else {
        return None;
    };
    let arch = if cfg!(target_arch = "x86_64") {
        Arch::Amd64
    } else if cfg!(target_arch = "aarch64") {
        Arch::Arm64
    } else {
        return None;
    };
    Some(PlatformKey { os, arch })
}

pub fn os_str(os: Os) -> &'static str {
    match os {
        Os::Linux => "linux",
        Os::Darwin => "darwin",
    }
}

pub fn arch_str(arch: Arch) -> &'static str {
    match arch {
        Arch::Amd64 => "amd64",
        Arch::Arm64 => "arm64",
    }
}
