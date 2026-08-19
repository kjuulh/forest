//! DATA-583 — `FOREST_COMPONENT_VERSION` overrides the manifest version.
//!
//! This is the build half of the version-from-tag contract. `forest publish
//! --version` moves what the *registry* records; this moves what gets *stamped
//! into the binary* (`-ldflags -X main.version=…` for Go, the same variable for
//! Rust). If only one of the two moved, a tag-triggered release would ship an
//! artifact that misreports its own version — the exact inconsistency the
//! feature exists to remove.
//!
//! Deliberately its own test binary. The override is read from the process
//! environment, and cargo runs the tests within a binary on threads with no
//! serialisation, so a test that mutates the environment corrupts whichever
//! tests happen to be running beside it. One test per process is the only
//! arrangement that makes the mutation safe without a lock.

use forest_build_core::{Toolchain, manifest::read_build_request};

fn have_cue() -> bool {
    std::process::Command::new("cue")
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Self-contained manifest — no `forest.sh/...` import, so `cue export` needs
/// no registry and the test runs offline. Mirrors the shape the real reader
/// consumes.
fn write_manifest(root: &std::path::Path, version: &str) {
    std::fs::write(
        root.join("forest.cue"),
        format!(
            "forest: component: {{\n\tname: \"widget\"\n\tversion: \"{version}\"\n\tupload: {{\n\t\ttype: \"go\"\n\t\tsource: \".\"\n\t\tarchitectures: {{ linux: {{ amd64: {{}} }} }}\n\t}}\n}}\n"
        ),
    )
    .unwrap();
}

#[tokio::test]
async fn env_overrides_the_manifest_version_and_blanks_fall_back() {
    if !have_cue() {
        eprintln!("skipping: `cue` not available");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_manifest(root, "0.1.7");

    // Unset: the manifest wins, which is the pre-DATA-583 behaviour and what
    // every manual build must keep doing.
    //
    // SAFETY for every mutation in this test: this binary contains exactly one
    // test, so nothing else is reading the environment concurrently.
    unsafe { std::env::remove_var("FOREST_COMPONENT_VERSION") };
    let req = read_build_request(Toolchain::Golang, root).await.unwrap();
    assert_eq!(req.version, "0.1.7", "manifest version when env is unset");

    // Set: the environment wins, so the build stamps the tag's version.
    unsafe { std::env::set_var("FOREST_COMPONENT_VERSION", "0.1.99-ci.1") };
    let req = read_build_request(Toolchain::Golang, root).await.unwrap();
    assert_eq!(
        req.version, "0.1.99-ci.1",
        "env should override the manifest"
    );

    // Blank: an unset CI input interpolates to the empty string. That has to
    // mean "no override" — stamping "" would produce a binary reporting nothing
    // at all, and would fail the publish-side semver gate.
    unsafe { std::env::set_var("FOREST_COMPONENT_VERSION", "") };
    let req = read_build_request(Toolchain::Golang, root).await.unwrap();
    assert_eq!(req.version, "0.1.7", "blank env must fall back to manifest");

    // Whitespace is trimmed, so an interpolated tag with a trailing newline
    // still lands as a clean semver rather than failing validation later.
    unsafe { std::env::set_var("FOREST_COMPONENT_VERSION", "  0.2.0\n") };
    let req = read_build_request(Toolchain::Golang, root).await.unwrap();
    assert_eq!(req.version, "0.2.0", "env override should be trimmed");

    unsafe { std::env::remove_var("FOREST_COMPONENT_VERSION") };
}
