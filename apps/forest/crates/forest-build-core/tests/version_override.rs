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

    // SAFETY for every mutation in this test: this binary contains exactly one
    // test, so nothing else is reading the environment concurrently.
    unsafe { std::env::remove_var("FOREST_COMPONENT_VERSION") };
    let req = read_build_request(Toolchain::Golang, root).await.unwrap();
    assert_eq!(req.version, "0.1.7", "manifest version when env is unset");

    unsafe { std::env::set_var("FOREST_COMPONENT_VERSION", "0.1.99-ci.1") };
    let req = read_build_request(Toolchain::Golang, root).await.unwrap();
    assert_eq!(
        req.version, "0.1.99-ci.1",
        "env should override the manifest"
    );

    unsafe { std::env::set_var("FOREST_COMPONENT_VERSION", "") };
    let req = read_build_request(Toolchain::Golang, root).await.unwrap();
    assert_eq!(req.version, "0.1.7", "blank env must fall back to manifest");

    unsafe { std::env::set_var("FOREST_COMPONENT_VERSION", "  0.2.0\n") };
    let req = read_build_request(Toolchain::Golang, root).await.unwrap();
    assert_eq!(req.version, "0.2.0", "env override should be trimmed");

    unsafe { std::env::remove_var("FOREST_COMPONENT_VERSION") };
}
