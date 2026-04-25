//! Environment-driven harness configuration.

use std::path::PathBuf;

/// All paths and remote endpoints the harness needs.
///
/// Read from environment variables so `cargo test` can pick them up without
/// arguments. If `HOLLOW_TEST_HOST` is unset, the harness can't run and tests
/// should skip gracefully.
#[derive(Debug, Clone)]
pub struct Config {
    /// SSH target, e.g. `user@host` or an alias from `~/.ssh/config`.
    pub host: String,
    /// Optional explicit identity file (passed to ssh as `-i`).
    pub ssh_key: Option<PathBuf>,
    /// Working directory on the remote host. Created on first run.
    pub remote_dir: PathBuf,
    /// Pinned Firecracker release used by the bootstrap.
    pub firecracker_version: &'static str,
    /// SHA256 of the Firecracker release tarball (x86_64). Verified after
    /// download before we install jailer/firecracker. Update alongside
    /// `firecracker_version`.
    pub firecracker_tarball_sha256: &'static str,
    /// Pinned kernel artifact path inside the official Firecracker CI bucket.
    pub kernel_s3_key: &'static str,
    /// Local repository root (resolved from `CARGO_MANIFEST_DIR` of this crate).
    pub repo_root: PathBuf,
    /// Where to put built artifacts on the dev machine.
    pub local_target_dir: PathBuf,
}

impl Config {
    /// Returns `None` if `HOLLOW_TEST_HOST` is unset — tests should treat that
    /// as a skip, not a failure.
    pub fn from_env() -> Option<Self> {
        let host = std::env::var("HOLLOW_TEST_HOST").ok()?;
        let ssh_key = std::env::var("HOLLOW_TEST_KEY").ok().map(PathBuf::from);
        let remote_dir = std::env::var("HOLLOW_TEST_REMOTE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/var/lib/hollow-test"));

        // `hollow-test-harness/Cargo.toml` ↦ `hollow/crates/hollow-test-harness/`,
        // so two levels up is the hollow workspace root.
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()? // crates
            .parent()? // hollow
            .to_path_buf();
        let local_target_dir = repo_root.join("target/test-harness");

        Some(Self {
            host,
            ssh_key,
            remote_dir,
            firecracker_version: "v1.15.1",
            // Published at github.com/firecracker-microvm/firecracker/releases/download/
            // v1.15.1/firecracker-v1.15.1-x86_64.tgz.sha256.txt
            firecracker_tarball_sha256:
                "d4a32ab2322d887ca1bc4a4e7afa9cc35393e6362dfc2b3becb389d362e4275a",
            kernel_s3_key: "firecracker-ci/20260408-ce2a467895c1-0/x86_64/vmlinux-6.1.166",
            repo_root,
            local_target_dir,
        })
    }

    /// Build the SSH command prefix (with key option if configured).
    pub fn ssh_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(key) = &self.ssh_key {
            args.push("-i".into());
            args.push(key.to_string_lossy().into());
        }
        args.push(self.host.clone());
        args
    }
}
