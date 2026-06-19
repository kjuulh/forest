//! Toolchain build logic shared by forest's build components (DATA-312).
//!
//! This crate owns the actual `cargo` / `go` / `docker` invocations that used
//! to live in `forest`'s bespoke `build` command. Forest the CLI does NOT
//! depend on this crate's build functions on its command path — they are
//! invoked by the per-toolchain build *components* (`forest-contrib/build-rust`
//! et al). Forest keeps owning cache / meta.json / descriptor bookkeeping; this
//! crate's only job is to turn source into an artifact on disk.
//!
//! Scope is deliberately just the compile step: resolve targets, shell out to
//! the toolchain (streaming output to stderr), and write a `checksums.sha256`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use sha2::{Digest, Sha256};

pub mod component;
pub mod manifest;

/// The toolchains a build component can drive. One component per variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toolchain {
    Rust,
    Golang,
    Docker,
}

/// A resolved build target: one (os, arch) pair plus the toolchain-specific
/// identifiers needed to drive the compiler.
#[derive(Debug)]
pub struct BuildTarget {
    pub os: String,
    pub arch: String,
    pub rust_target: Option<String>,
    pub go_os: Option<String>,
    pub go_arch: Option<String>,
    pub docker_platform: Option<String>,
}

/// One built artifact, reported back in the build summary.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BuiltArtifact {
    pub os: String,
    pub arch: String,
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
}

/// The JSON summary a build component returns on success.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BuildSummary {
    pub name: String,
    pub version: String,
    pub artifacts: Vec<BuiltArtifact>,
}

/// End-to-end build for a build component: read the project manifest from
/// `work_dir`, resolve targets, compile each, write `checksums.sha256`, and
/// return a summary. Artifacts land in `<work_dir>/.forest/component/output`
/// and the cargo target dir — forest's publish/run paths pick them up from
/// there (this crate intentionally does NOT touch forest's component cache or
/// meta.json). DATA-312.
pub async fn run_build(toolchain: Toolchain, work_dir: &Path) -> anyhow::Result<BuildSummary> {
    let req = manifest::read_build_request(toolchain, work_dir).await?;

    let targets = resolve_targets(&req.architectures, toolchain)?;
    if targets.is_empty() {
        anyhow::bail!("no build targets resolved from architectures");
    }

    tracing::info!(
        "building {} target(s) for component '{}'",
        targets.len(),
        req.name,
    );

    for target in &targets {
        tracing::info!("building {}/{} ...", target.os, target.arch);
        build_target(
            toolchain,
            &req.name,
            &req.version,
            &req.source,
            &req.out_base,
            target,
        )
        .await?;
    }

    generate_checksums(&req.name, &targets, &req.out_base)?;

    let mut artifacts = Vec::new();
    for target in &targets {
        let path = output_dir(&req.out_base, &target.os, &target.arch)?
            .join(output_filename(&req.name, target));
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read built artifact {}", path.display()))?;
        artifacts.push(BuiltArtifact {
            os: target.os.clone(),
            arch: target.arch.clone(),
            sha256: hex::encode(Sha256::digest(&bytes)),
            size: bytes.len() as u64,
            path,
        });
    }

    Ok(BuildSummary {
        name: req.name,
        version: req.version,
        artifacts,
    })
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

/// Resolve build targets from the CUE `upload.architectures` map
/// (`{os: {arch: {}}}`) for the given toolchain.
pub fn resolve_targets(
    architectures: &HashMap<String, HashMap<String, serde_json::Value>>,
    toolchain: Toolchain,
) -> anyhow::Result<Vec<BuildTarget>> {
    let mut targets = Vec::new();

    for (os, arches) in architectures {
        for arch in arches.keys() {
            let mut target = BuildTarget {
                os: os.clone(),
                arch: arch.clone(),
                rust_target: None,
                go_os: None,
                go_arch: None,
                docker_platform: None,
            };

            match toolchain {
                Toolchain::Rust => {
                    target.rust_target = Some(rust_target_triple(os, arch)?);
                }
                Toolchain::Golang => {
                    let (go_os, go_arch) = golang_target(os, arch)?;
                    target.go_os = Some(go_os);
                    target.go_arch = Some(go_arch);
                }
                Toolchain::Docker => {
                    target.docker_platform = Some(docker_platform(os, arch)?);
                }
            }

            targets.push(target);
        }
    }

    // Sort for deterministic build order.
    targets.sort_by(|a, b| (&a.os, &a.arch).cmp(&(&b.os, &b.arch)));
    Ok(targets)
}

/// `<out_base>/<os>/<arch>/`, created if absent.
pub fn output_dir(out_base: &Path, os: &str, arch: &str) -> anyhow::Result<PathBuf> {
    let dir = out_base.join(format!("{os}/{arch}/"));
    std::fs::create_dir_all(&dir).context("failed to create output dir")?;
    Ok(dir)
}

/// Artifact filename for a target: `<name>.tar` for docker, `<name>.exe` for
/// windows, bare `<name>` otherwise.
pub fn output_filename(component_name: &str, target: &BuildTarget) -> String {
    if target.docker_platform.is_some() {
        format!("{component_name}.tar")
    } else if target.os == "windows" {
        format!("{component_name}.exe")
    } else {
        component_name.to_string()
    }
}

/// Write `<out_base>/checksums.sha256` covering every target's artifact.
pub fn generate_checksums(
    component_name: &str,
    targets: &[BuildTarget],
    out_base: &Path,
) -> anyhow::Result<()> {
    let mut entries = Vec::new();

    for target in targets {
        let filename = output_filename(component_name, target);
        let rel_path = format!("{}/{}/{}", target.os, target.arch, filename);
        let abs_path = out_base.join(&rel_path);

        let bytes = std::fs::read(&abs_path)
            .with_context(|| format!("failed to read artifact for checksum: {rel_path}"))?;

        let hash = Sha256::digest(&bytes);
        entries.push(format!("{}  {}", hex::encode(hash), rel_path));
    }

    entries.sort();

    let checksums_path = out_base.join("checksums.sha256");
    let content = entries.join("\n") + "\n";
    std::fs::write(&checksums_path, &content).context("failed to write checksums.sha256")?;

    tracing::info!("wrote {}", checksums_path.display());
    Ok(())
}

/// Build a single target with the given toolchain. `source` is the directory to
/// run the build in; artifacts land under `out_base/<os>/<arch>/`.
pub async fn build_target(
    toolchain: Toolchain,
    component_name: &str,
    component_version: &str,
    source: &Path,
    out_base: &Path,
    target: &BuildTarget,
) -> anyhow::Result<()> {
    match toolchain {
        Toolchain::Rust => build_rust(component_name, source, out_base, target).await,
        Toolchain::Golang => build_golang(component_name, source, out_base, target).await,
        Toolchain::Docker => {
            build_docker(component_name, component_version, source, out_base, target).await
        }
    }
}

async fn build_rust(
    component_name: &str,
    source: &Path,
    out_base: &Path,
    target: &BuildTarget,
) -> anyhow::Result<()> {
    let triple = target
        .rust_target
        .as_ref()
        .context("rust target not resolved")?;

    let out_dir = output_dir(out_base, &target.os, &target.arch)?;

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
        .arg(format!("--bin={component_name}"))
        .arg(format!("--artifact-dir={}", out_dir.display()))
        .arg("-Z")
        .arg("unstable-options");

    cmd.stdout(std::io::stdout());
    // TASKS/031: capture stderr so we can pattern-match it for actionable
    // hints on failure. We still tee it to the user's terminal in real time
    // so build output isn't blocked behind the full child exit.
    cmd.stderr(std::process::Stdio::piped());

    let mut proc = cmd.spawn()?;
    let captured_stderr = if let Some(stderr) = proc.stderr.take() {
        Some(tee_stderr(stderr).await?)
    } else {
        None
    };
    let exit = proc.wait().await?;

    if !exit.success() {
        let stderr_text = captured_stderr.as_deref().unwrap_or("");
        emit_rust_build_hints(stderr_text, triple);
        anyhow::bail!(
            "failed to build rust component for {}/{}",
            target.os,
            target.arch,
        );
    }

    Ok(())
}

/// Drain a child's stderr to our own stderr line by line while accumulating
/// the full text. Returns once EOF is reached. Used so we can match cargo's
/// error output for actionable hints (TASKS/031) without losing the live
/// streaming behaviour users expect from cargo.
async fn tee_stderr(stderr: tokio::process::ChildStderr) -> anyhow::Result<String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut reader = BufReader::new(stderr).lines();
    let mut buffer = String::new();
    let mut user_stderr = tokio::io::stderr();
    while let Some(line) = reader.next_line().await? {
        buffer.push_str(&line);
        buffer.push('\n');
        user_stderr.write_all(line.as_bytes()).await?;
        user_stderr.write_all(b"\n").await?;
    }
    user_stderr.flush().await?;
    Ok(buffer)
}

/// Pattern-driven build hints. Each rule matches a substring of cargo's
/// stderr and emits a targeted hint. Multiple matching rules ALL fire so
/// users get every relevant pointer. If no rule matches, no hint is emitted —
/// silence beats a misleading default (TASKS/031, items #3 + #9).
struct HintRule {
    pattern: &'static str,
    hint: &'static str,
}

const BUILD_HINT_RULES: &[HintRule] = &[
    HintRule {
        pattern: "no bin target named",
        hint: "hint: forest invokes `cargo build --bin <component-name>` for Rust\n      \
               components. The cargo [[bin]] name (or implicit package.name when no\n      \
               [[bin]] is declared) must match forest.component.name. Rename one\n      \
               so they agree.",
    },
    HintRule {
        pattern: "linker `cc` not found",
        hint: "hint: install a C linker (Xcode Command Line Tools on macOS,\n      \
               `build-essential` on Debian/Ubuntu).",
    },
    HintRule {
        pattern: "linker `link.exe` not found",
        hint: "hint: install MSVC build tools on Windows (rustup-init suggests this).",
    },
    HintRule {
        pattern: "could not find native static library",
        hint: "hint: a system library required by a Rust dependency is missing.\n      \
               Check the failing crate's README for required system packages.",
    },
    HintRule {
        pattern: "may not be installed",
        hint: "hint: the target toolchain is missing. Run:\n  rustup target add --toolchain nightly <triple>",
    },
    HintRule {
        pattern: "the target may not be installed",
        hint: "hint: the target toolchain is missing. Run:\n  rustup target add --toolchain nightly <triple>",
    },
    HintRule {
        pattern: "wasm32",
        hint: "hint: for wasm32 builds you may need the wasm32 target installed\n      \
               and `wasm-ld` on PATH.",
    },
];

fn emit_rust_build_hints(stderr: &str, triple: &str) {
    let mut emitted_any = false;
    for rule in BUILD_HINT_RULES {
        if stderr.contains(rule.pattern) {
            if !emitted_any {
                eprintln!();
                emitted_any = true;
            }
            // Substitute the placeholder for the actual triple where applicable.
            eprintln!("{}", rule.hint.replace("<triple>", triple));
        }
    }
    if emitted_any {
        eprintln!();
    }
}

async fn build_golang(
    component_name: &str,
    source: &Path,
    out_base: &Path,
    target: &BuildTarget,
) -> anyhow::Result<()> {
    let go_os = target.go_os.as_ref().context("go os not resolved")?;
    let go_arch = target.go_arch.as_ref().context("go arch not resolved")?;

    let out_dir = output_dir(out_base, &target.os, &target.arch)?;

    let bin_name = if target.os == "windows" {
        format!("{component_name}.exe")
    } else {
        component_name.to_string()
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
    component_name: &str,
    component_version: &str,
    source: &Path,
    out_base: &Path,
    target: &BuildTarget,
) -> anyhow::Result<()> {
    let platform = target
        .docker_platform
        .as_ref()
        .context("docker platform not resolved")?;

    ensure_buildx_builder().await?;

    let out_dir = output_dir(out_base, &target.os, &target.arch)?;
    let tar_name = format!("{component_name}.tar");
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
        &format!("{component_name}:{component_version}"),
        ".",
    ]);

    cmd.stdout(std::io::stdout());
    cmd.stderr(std::io::stderr());

    let mut proc = cmd
        .spawn()
        .context("failed to spawn docker buildx — is docker with buildx installed?")?;
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

#[cfg(test)]
mod build_hint_tests {
    use super::*;

    fn capture_hints(stderr: &str, triple: &str) -> String {
        // Re-run the matching logic and accumulate into a string, since the
        // production code writes to the real stderr. Mirror the iteration
        // order and substitution exactly.
        let mut out = String::new();
        for rule in BUILD_HINT_RULES {
            if stderr.contains(rule.pattern) {
                out.push_str(&rule.hint.replace("<triple>", triple));
                out.push('\n');
            }
        }
        out
    }

    #[test]
    fn no_bin_target_triggers_name_alignment_hint() {
        let stderr = "error: no bin target named `canopy-data-cli` in default-run packages\n\
                      available bin targets: data";
        let hints = capture_hints(stderr, "aarch64-apple-darwin");
        assert!(
            hints.contains("must match forest.component.name"),
            "expected name-alignment hint, got: {hints}"
        );
        // Critical regression guard: the misleading cross-compile hint must
        // NOT show up for this category of error.
        assert!(
            !hints.contains("rustup target add"),
            "no_bin_target must not trigger the cross-compile hint, got: {hints}"
        );
    }

    #[test]
    fn missing_linker_triggers_install_hint() {
        let stderr = "error: linker `cc` not found";
        let hints = capture_hints(stderr, "x86_64-unknown-linux-gnu");
        assert!(hints.contains("Xcode Command Line Tools"));
    }

    #[test]
    fn missing_target_triggers_rustup_hint_with_substituted_triple() {
        let stderr = "error: the target may not be installed";
        let hints = capture_hints(stderr, "wasm32-unknown-unknown");
        assert!(hints.contains("rustup target add --toolchain nightly wasm32-unknown-unknown"));
    }

    #[test]
    fn unrelated_error_produces_no_hint() {
        let stderr = "error[E0599]: no method named `foo` found";
        let hints = capture_hints(stderr, "aarch64-apple-darwin");
        assert!(hints.is_empty(), "expected no hints, got: {hints}");
    }

    #[test]
    fn empty_stderr_produces_no_hint() {
        let hints = capture_hints("", "aarch64-apple-darwin");
        assert!(hints.is_empty());
    }

    #[test]
    fn multiple_matching_rules_all_fire() {
        // Synthetic case: stderr contains both a linker error and a target
        // error. We want BOTH hints, not just the first matching one.
        let stderr = "error: linker `cc` not found\n\
                      error: the target may not be installed";
        let hints = capture_hints(stderr, "aarch64-apple-darwin");
        assert!(hints.contains("Xcode Command Line Tools"));
        assert!(hints.contains("rustup target add"));
    }

    #[test]
    fn resolve_targets_rust_sorts_deterministically() {
        let mut arches = HashMap::new();
        let mut linux = HashMap::new();
        linux.insert("arm64".to_string(), serde_json::json!({}));
        linux.insert("amd64".to_string(), serde_json::json!({}));
        arches.insert("linux".to_string(), linux);
        let targets = resolve_targets(&arches, Toolchain::Rust).unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(
            (targets[0].os.as_str(), targets[0].arch.as_str()),
            ("linux", "amd64")
        );
        assert!(targets[0].rust_target.is_some());
    }
}
