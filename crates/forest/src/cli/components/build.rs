use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::state::State;

#[derive(clap::Parser)]
pub struct BuildCommand {}

impl BuildCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        let mut cmd = tokio::process::Command::new("cue");
        cmd.args(["export", "./forest.cue", "./spec.cue", "--out", "json"]);

        let output = cmd.output().await?;
        let stdout = String::from_utf8(output.stdout)?;
        let stderr = String::from_utf8(output.stderr)?;

        if !output.status.success() {
            anyhow::bail!("failed to spec from cue: {stdout}, {stderr}");
        }

        let doc: Document = serde_json::from_str(stdout.trim())?;

        let Some(component) = &doc.forest.as_ref().and_then(|f| f.component.as_ref()) else {
            anyhow::bail!("cannot build when no forest.component section is set");
        };

        let Some(upload) = &component.upload else {
            anyhow::bail!("forest.component.upload section is required for building");
        };

        let architectures = upload
            .architectures
            .as_ref()
            .context("architectures section is required for building")?;

        let targets = resolve_targets(architectures, &upload.source_type)?;

        if targets.is_empty() {
            anyhow::bail!("no build targets resolved from architectures");
        }

        tracing::info!(
            "building {} target(s) for component '{}'",
            targets.len(),
            component.name,
        );

        for target in &targets {
            tracing::info!("building {}/{} ...", target.os, target.arch);

            match upload.source_type {
                SourceType::Rust => {
                    build_rust(state, component, &upload.source, target).await?;
                }
                SourceType::Golang => {
                    build_golang(state, component, &upload.source, target).await?;
                }
                SourceType::Docker => {
                    build_docker(state, component, &upload.source, target).await?;
                }
            }
        }

        generate_checksums(&component.name, &targets)?;

        tracing::info!("all targets built successfully");
        Ok(())
    }
}

#[derive(Debug)]
struct BuildTarget {
    os: String,
    arch: String,
    rust_target: Option<String>,
    go_os: Option<String>,
    go_arch: Option<String>,
    docker_platform: Option<String>,
}

fn rust_target_triple(os: &str, arch: &str) -> anyhow::Result<String> {
    let triple = match (os, arch) {
        ("linux", "amd64") => "x86_64-unknown-linux-gnu",
        ("linux", "arm64") => "aarch64-unknown-linux-gnu",
        ("macos", "amd64") => "x86_64-apple-darwin",
        ("macos", "arm64") => "aarch64-apple-darwin",
        ("windows", "amd64") => "x86_64-pc-windows-msvc",
        ("windows", "arm64") => "aarch64-pc-windows-msvc",
        _ => anyhow::bail!("unsupported rust target: {os}/{arch}"),
    };
    Ok(triple.to_string())
}

fn golang_target(os: &str, arch: &str) -> anyhow::Result<(String, String)> {
    let goos = match os {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "windows",
        _ => anyhow::bail!("unsupported go os: {os}"),
    };
    let goarch = match arch {
        "amd64" => "amd64",
        "arm64" => "arm64",
        _ => anyhow::bail!("unsupported go arch: {arch}"),
    };
    Ok((goos.to_string(), goarch.to_string()))
}

fn docker_platform(os: &str, arch: &str) -> anyhow::Result<String> {
    let plat_os = match os {
        "linux" => "linux",
        _ => anyhow::bail!("unsupported docker os: {os} (docker builds only support linux)"),
    };
    let plat_arch = match arch {
        "amd64" => "amd64",
        "arm64" => "arm64",
        _ => anyhow::bail!("unsupported docker arch: {arch}"),
    };
    Ok(format!("{plat_os}/{plat_arch}"))
}

fn resolve_targets(
    architectures: &HashMap<String, HashMap<String, serde_json::Value>>,
    source_type: &SourceType,
) -> anyhow::Result<Vec<BuildTarget>> {
    let mut targets = Vec::new();

    for (os, arches) in architectures {
        for (arch, _) in arches {
            let mut target = BuildTarget {
                os: os.clone(),
                arch: arch.clone(),
                rust_target: None,
                go_os: None,
                go_arch: None,
                docker_platform: None,
            };

            match source_type {
                SourceType::Rust => {
                    target.rust_target = Some(rust_target_triple(os, arch)?);
                }
                SourceType::Golang => {
                    let (go_os, go_arch) = golang_target(os, arch)?;
                    target.go_os = Some(go_os);
                    target.go_arch = Some(go_arch);
                }
                SourceType::Docker => {
                    target.docker_platform = Some(docker_platform(os, arch)?);
                }
            }

            targets.push(target);
        }
    }

    // Sort for deterministic build order
    targets.sort_by(|a, b| (&a.os, &a.arch).cmp(&(&b.os, &b.arch)));
    Ok(targets)
}

fn output_base_dir() -> anyhow::Result<PathBuf> {
    let cur_dir = std::env::current_dir()?;
    Ok(cur_dir.join(".forest/component/output"))
}

fn output_dir(os: &str, arch: &str) -> anyhow::Result<PathBuf> {
    let dir = output_base_dir()?.join(format!("{os}/{arch}/"));
    std::fs::create_dir_all(&dir).context("failed to create output dir")?;
    Ok(dir)
}

fn output_filename(component_name: &str, target: &BuildTarget) -> String {
    if target.docker_platform.is_some() {
        format!("{component_name}.tar")
    } else if target.os == "windows" {
        format!("{component_name}.exe")
    } else {
        component_name.to_string()
    }
}

fn generate_checksums(component_name: &str, targets: &[BuildTarget]) -> anyhow::Result<()> {
    let base = output_base_dir()?;
    let mut entries = Vec::new();

    for target in targets {
        let filename = output_filename(component_name, target);
        let rel_path = format!("{}/{}/{}", target.os, target.arch, filename);
        let abs_path = base.join(&rel_path);

        let bytes = std::fs::read(&abs_path)
            .with_context(|| format!("failed to read artifact for checksum: {rel_path}"))?;

        let hash = Sha256::digest(&bytes);
        entries.push(format!("{}  {}", hex::encode(hash), rel_path));
    }

    entries.sort();

    let checksums_path = base.join("checksums.sha256");
    let content = entries.join("\n") + "\n";
    std::fs::write(&checksums_path, &content)
        .context("failed to write checksums.sha256")?;

    tracing::info!("wrote {}", checksums_path.display());
    Ok(())
}

async fn build_rust(
    _state: &State,
    component: &Component,
    source: &Path,
    target: &BuildTarget,
) -> anyhow::Result<()> {
    let triple = target
        .rust_target
        .as_ref()
        .context("rust target not resolved")?;

    let out_dir = output_dir(&target.os, &target.arch)?;

    tracing::info!(
        "building rust project: {} (target: {triple})",
        source.display()
    );

    let mut cmd = tokio::process::Command::new("cargo");
    cmd.current_dir(source);
    cmd.arg("+nightly")
        .arg("build")
        .arg("--release")
        .arg(format!("--target={triple}"))
        .arg(format!("--bin={}", component.name))
        .arg(format!("--artifact-dir={}", out_dir.display()))
        .arg("-Z")
        .arg("unstable-options");

    cmd.stdout(std::io::stdout());
    cmd.stderr(std::io::stderr());

    let mut proc = cmd.spawn()?;
    let exit = proc.wait().await?;

    if !exit.success() {
        eprintln!();
        eprintln!("hint: if the error mentions the target may not be installed, run:");
        eprintln!();
        eprintln!("  rustup target add --toolchain nightly {triple}");
        eprintln!();
        eprintln!("hint: for cross-compilation you may also need the appropriate");
        eprintln!("      linker and sysroot configured in .cargo/config.toml");
        eprintln!();
        anyhow::bail!(
            "failed to build rust component for {}/{}",
            target.os,
            target.arch,
        );
    }

    Ok(())
}

async fn build_golang(
    _state: &State,
    component: &Component,
    source: &Path,
    target: &BuildTarget,
) -> anyhow::Result<()> {
    let go_os = target.go_os.as_ref().context("go os not resolved")?;
    let go_arch = target.go_arch.as_ref().context("go arch not resolved")?;

    let out_dir = output_dir(&target.os, &target.arch)?;

    let bin_name = if target.os == "windows" {
        format!("{}.exe", component.name)
    } else {
        component.name.clone()
    };

    let output_path = out_dir.join(&bin_name);

    tracing::info!(
        "building go project: {} (GOOS={go_os} GOARCH={go_arch})",
        source.display()
    );

    let mut cmd = tokio::process::Command::new("go");
    cmd.current_dir(source);
    cmd.env("GOOS", go_os);
    cmd.env("GOARCH", go_arch);
    cmd.env("CGO_ENABLED", "0");
    cmd.args(["build", "-o"]);
    cmd.arg(&output_path);
    cmd.arg(".");

    cmd.stdout(std::io::stdout());
    cmd.stderr(std::io::stderr());

    let mut proc = cmd.spawn()?;
    let exit = proc.wait().await?;

    if !exit.success() {
        anyhow::bail!(
            "failed to build go component for {}/{}",
            target.os,
            target.arch,
        );
    }

    Ok(())
}

const FOREST_BUILDER_NAME: &str = "forest-builder";

async fn ensure_buildx_builder() -> anyhow::Result<()> {
    let inspect = tokio::process::Command::new("docker")
        .args(["buildx", "inspect", FOREST_BUILDER_NAME])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await?;

    if inspect.success() {
        return Ok(());
    }

    tracing::info!("creating buildx builder '{FOREST_BUILDER_NAME}' (docker-container driver)");

    let create = tokio::process::Command::new("docker")
        .args([
            "buildx",
            "create",
            "--name",
            FOREST_BUILDER_NAME,
            "--driver",
            "docker-container",
            "--bootstrap",
        ])
        .stdout(std::io::stdout())
        .stderr(std::io::stderr())
        .status()
        .await?;

    if !create.success() {
        anyhow::bail!("failed to create buildx builder '{FOREST_BUILDER_NAME}'");
    }

    Ok(())
}

async fn build_docker(
    _state: &State,
    component: &Component,
    source: &Path,
    target: &BuildTarget,
) -> anyhow::Result<()> {
    let platform = target
        .docker_platform
        .as_ref()
        .context("docker platform not resolved")?;

    ensure_buildx_builder().await?;

    let out_dir = output_dir(&target.os, &target.arch)?;
    let tar_name = format!("{}.tar", component.name);
    let output_path = out_dir.join(&tar_name);

    tracing::info!(
        "building docker image: {} (platform: {platform})",
        source.display()
    );

    let mut cmd = tokio::process::Command::new("docker");
    cmd.current_dir(source);
    cmd.args([
        "buildx",
        "build",
        "--builder",
        FOREST_BUILDER_NAME,
        "--platform",
        platform,
        "--output",
        &format!("type=docker,dest={}", output_path.display()),
        "-t",
        &format!("{}:{}", component.name, component.version),
        ".",
    ]);

    cmd.stdout(std::io::stdout());
    cmd.stderr(std::io::stderr());

    let mut proc = cmd.spawn().context(
        "failed to spawn docker buildx — is docker with buildx installed?",
    )?;
    let exit = proc.wait().await?;

    if !exit.success() {
        eprintln!();
        eprintln!("hint: make sure docker buildx is available:");
        eprintln!();
        eprintln!("  docker buildx version");
        eprintln!();
        eprintln!("hint: for cross-platform builds you may need QEMU emulation:");
        eprintln!();
        eprintln!("  docker run --rm --privileged multiarch/qemu-user-static --reset -p yes");
        eprintln!();
        anyhow::bail!(
            "failed to build docker image for {}/{}",
            target.os,
            target.arch,
        );
    }

    Ok(())
}

// --- Models ---

#[derive(Debug, Serialize, Deserialize)]
pub struct Document {
    forest: Option<Forest>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Forest {
    pub component: Option<Component>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Component {
    pub name: String,
    pub version: String,
    pub codegen: Option<Codegen>,
    pub upload: Option<Upload>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Codegen {
    #[serde(rename = "type")]
    pub source_type: SourceType,
    pub output: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Upload {
    #[serde(rename = "type")]
    pub source_type: SourceType,
    pub source: PathBuf,
    pub registry: String,
    pub architectures: Option<HashMap<String, HashMap<String, serde_json::Value>>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum SourceType {
    #[serde(rename = "rust")]
    Rust,
    #[serde(rename = "go")]
    Golang,
    #[serde(rename = "docker")]
    Docker,
}
