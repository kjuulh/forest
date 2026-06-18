//! End-to-end smoke for the build path (DATA-312).
//!
//! Drives the real `run_build` against a throwaway cargo project with a
//! self-contained CUE manifest (no registry import), so it runs fully offline
//! — proving target resolution, manifest reading, the actual `cargo` invocation,
//! and the summary all hang together. The same code path the `build-rust`
//! component runs in production.
//!
//! Skips gracefully when `cargo +nightly` or `cue` aren't available, matching
//! the repo's convention for tool-dependent tests.

use std::path::Path;
use std::process::Command;

use forest_build_core::{Toolchain, run_build};

fn have(bin: &str, args: &[&str]) -> bool {
    Command::new(bin)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Map the host to forest's (os, arch) naming so the build is native (no
/// cross-compile toolchain required).
fn host_os_arch() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "windows",
        other => panic!("unsupported test host os: {other}"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => panic!("unsupported test host arch: {other}"),
    };
    (os, arch)
}

#[tokio::test]
async fn run_build_compiles_a_rust_component() {
    if !have("cargo", &["+nightly", "--version"]) {
        eprintln!("skipping: `cargo +nightly` not available");
        return;
    }
    if !have("cue", &["version"]) {
        eprintln!("skipping: `cue` not available");
        return;
    }

    let (os, arch) = host_os_arch();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let name = "forest-smoke";

    // A minimal, dependency-free cargo binary whose name matches the component.
    std::fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"{name}\"\npath = \"main.rs\"\n"
        ),
    )
    .unwrap();
    std::fs::write(root.join("main.rs"), "fn main() { println!(\"hi\"); }\n").unwrap();

    // Self-contained CUE manifest — no `forest.sh/...` import, so `cue export`
    // needs no registry. Mirrors the shape run_build reads.
    std::fs::write(
        root.join("forest.cue"),
        format!(
            "forest: component: {{\n\tname: \"{name}\"\n\tversion: \"0.1.0\"\n\tupload: {{\n\t\ttype: \"rust\"\n\t\tsource: \".\"\n\t\tarchitectures: {{\n\t\t\t{os}: {{\n\t\t\t\t{arch}: {{}}\n\t\t\t}}\n\t\t}}\n\t}}\n}}\n"
        ),
    )
    .unwrap();

    let summary = run_build(Toolchain::Rust, root)
        .await
        .expect("run_build should succeed");

    assert_eq!(summary.name, name);
    assert_eq!(summary.artifacts.len(), 1, "one platform declared");
    let artifact = &summary.artifacts[0];
    assert_eq!((artifact.os.as_str(), artifact.arch.as_str()), (os, arch));
    assert!(artifact.size > 0, "artifact should be non-empty");
    assert_eq!(artifact.sha256.len(), 64, "sha256 hex");
    assert!(
        Path::new(&artifact.path).is_file(),
        "artifact must exist on disk: {}",
        artifact.path.display()
    );
    // checksums.sha256 is written alongside the output tree.
    assert!(
        root.join(".forest/component/output/checksums.sha256").is_file(),
        "checksums.sha256 should be written"
    );
}

#[tokio::test]
async fn run_build_rejects_mismatched_toolchain() {
    if !have("cue", &["version"]) {
        eprintln!("skipping: `cue` not available");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // Declares a go upload, but we drive the Rust toolchain → should refuse.
    std::fs::write(
        root.join("forest.cue"),
        "forest: component: {\n\tname: \"x\"\n\tversion: \"0.1.0\"\n\tupload: {\n\t\ttype: \"go\"\n\t\tsource: \".\"\n\t\tarchitectures: { linux: { amd64: {} } }\n\t}\n}\n",
    )
    .unwrap();

    let err = run_build(Toolchain::Rust, root)
        .await
        .expect_err("toolchain mismatch must fail");
    let msg = format!("{err:#}");
    assert!(msg.contains("rust"), "msg: {msg}");
    assert!(msg.contains("go"), "msg: {msg}");
}
