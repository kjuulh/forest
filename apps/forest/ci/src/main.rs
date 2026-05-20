use std::path::PathBuf;

use clap::Parser;
const BIN_NAME: &str = "forest-server";
const MOLD_VERSION: &str = "2.40.4";
const SCCACHE_VERSION: &str = "0.9.1";
const SCCACHE_MEMCACHED_ENDPOINT: &str = "tcp://10.0.10.13:11211";
const MEMCACHED_METRICS_URL: &str = "http://10.0.10.13:9150/metrics";

struct PlatformSpec {
    platform: &'static str,
    rust_target: &'static str,
    /// Extra apt packages needed for cross-compilation (e.g. cross-linker).
    extra_apt_pkgs: &'static [&'static str],
    /// Extra env vars for cross-compilation (key, value pairs).
    extra_env: &'static [(&'static str, &'static str)],
    /// Whether this is a cross-compile target (skip mold, add rustup target).
    is_cross: bool,
}

const PLATFORM_AMD64: PlatformSpec = PlatformSpec {
    platform: "linux/amd64",
    rust_target: "x86_64-unknown-linux-gnu",
    extra_apt_pkgs: &[],
    extra_env: &[],
    is_cross: false,
};

const PLATFORM_ARM64: PlatformSpec = PlatformSpec {
    platform: "linux/arm64",
    rust_target: "aarch64-unknown-linux-gnu",
    extra_apt_pkgs: &["gcc-aarch64-linux-gnu"],
    extra_env: &[("CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER", "aarch64-linux-gnu-gcc")],
    is_cross: true,
};

#[derive(Parser)]
#[command(name = "ci")]
enum Cli {
    /// Run PR validation pipeline (check + test + build)
    Pr,
    /// Run main branch pipeline (check + test + build + publish)
    Main,
    /// Smoke-test Dagger service bindings (Postgres, NATS, MinIO)
    TestServices,
    // NOTE: the former `Release` / `ReleaseSnapshot` subcommands ran
    // GoReleaser inside Dagger to push Gitea releases. The forest CLI
    // is now distributed via GitHub Releases + Homebrew through
    // `.github/workflows/release.yml`, driven by release-please. The
    // server / forage Docker images still go out via ci.yaml here.
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Cli::parse();

    dagger_sdk::connect(|client| async move {
        match cli {
            Cli::Pr => run_pr(&client).await?,
            Cli::Main => run_main(&client).await?,
            Cli::TestServices => test_services(&client).await?,
        }
        Ok(())
    })
    .await?;

    Ok(())
}

async fn run_pr(client: &dagger_sdk::Query) -> eyre::Result<()> {
    eprintln!("==> PR pipeline: check + test + build");
    print_memcached_metrics(client, "before build").await?;

    let base = build_base(client).await?;

    eprintln!("--- cargo check --workspace");
    let check_output = cargo_with_stats(&base, "cargo check --workspace").stdout().await?;
    eprintln!("{check_output}");

    eprintln!("--- running tests");
    with_services(client, &base)
        .with_exec(vec![
            "cargo",
            "test",
            "--workspace",
            "--exclude",
            "forest-event-store",
            "--",
            "--skip",
            "component_flow",
        ])
        .sync()
        .await?;

    eprintln!("--- building release images (multi-platform)");
    let (_image_amd64, _image_arm64) = build_release_images(client).await?;

    print_memcached_metrics(client, "after build").await?;
    eprintln!("==> PR pipeline complete");
    Ok(())
}

async fn run_main(client: &dagger_sdk::Query) -> eyre::Result<()> {
    eprintln!("==> Main pipeline: check + test + build + publish");
    print_memcached_metrics(client, "before build").await?;

    let base = build_base(client).await?;

    eprintln!("--- cargo check --workspace");
    let check_output = cargo_with_stats(&base, "cargo check --workspace").stdout().await?;
    eprintln!("{check_output}");

    eprintln!("--- running tests");
    with_services(client, &base)
        .with_exec(vec![
            "cargo",
            "test",
            "--workspace",
            "--exclude",
            "forest-event-store",
            "--",
            "--skip",
            "component_flow",
        ])
        .sync()
        .await?;

    eprintln!("--- building release images (multi-platform)");
    let (image_amd64, image_arm64) = build_release_images(client).await?;

    eprintln!("--- publishing multi-platform image");
    publish_image(client, &image_amd64, &image_arm64).await?;

    print_memcached_metrics(client, "after build").await?;
    eprintln!("==> Main pipeline complete");
    Ok(())
}

/// Smoke-test that all service containers start and accept connections.
async fn test_services(client: &dagger_sdk::Query) -> eyre::Result<()> {
    eprintln!("==> Testing service connectivity");

    let base = client
        .container()
        .from("debian:trixie-slim")
        .with_exec(vec!["apt-get", "update", "-qq"])
        .with_exec(vec![
            "apt-get", "install", "-y", "-qq", "--no-install-recommends",
            "postgresql-client", "curl", "wget", "netcat-openbsd",
        ]);

    // --- post3 (S3) — test first to verify it works in Dagger ---
    eprintln!("--- testing post3 (S3)");
    let s3 = s3_service(client);
    base.clone()
        .with_service_binding("s3", s3)
        .with_exec(vec![
            "sh", "-c",
            "until nc -z s3 9000; do echo 'waiting for post3...'; sleep 1; done && \
             curl -sf -X PUT http://s3:9000/forest && \
             echo 'post3: OK (bucket created)'",
        ])
        .sync()
        .await?;
    eprintln!("--- post3: OK");

    // --- Postgres ---
    eprintln!("--- testing Postgres");
    let pg = postgres_service(client);
    base.clone()
        .with_service_binding("postgres", pg)
        .with_exec(vec![
            "sh", "-c",
            "until pg_isready -h postgres -U forest -d forest; do echo 'waiting for pg...'; sleep 1; done && \
             echo 'Postgres: OK'",
        ])
        .sync()
        .await?;
    eprintln!("--- Postgres: OK");

    // --- NATS ---
    eprintln!("--- testing NATS");
    let nats = nats_service(client);
    base.clone()
        .with_service_binding("nats", nats)
        .with_exec(vec![
            "sh", "-c",
            "until nc -z nats 4222; do echo 'waiting for nats...'; sleep 1; done && \
             echo 'NATS: OK'",
        ])
        .sync()
        .await?;
    eprintln!("--- NATS: OK");

    // --- All together ---
    eprintln!("--- testing all services together");
    let pg = postgres_service(client);
    let nats = nats_service(client);
    let s3 = s3_service(client);
    base.clone()
        .with_service_binding("postgres", pg)
        .with_service_binding("nats", nats)
        .with_service_binding("s3", s3)
        .with_env_variable("DATABASE_URL", "postgres://forest:forest@postgres:5432/forest")
        .with_env_variable("NATS_URL", "nats://nats:4222")
        .with_env_variable("S3_ENDPOINT", "http://s3:9000")
        .with_env_variable("S3_BUCKET", "forest")
        .with_env_variable("S3_ACCESS_KEY", "test")
        .with_env_variable("S3_SECRET_KEY", "test")
        .with_exec(vec![
            "sh", "-c",
            "until pg_isready -h postgres -U forest -d forest; do sleep 1; done && echo 'pg ok' && \
             until nc -z nats 4222; do sleep 1; done && echo 'nats ok' && \
             curl -sf -X PUT http://s3:9000/forest; echo 's3 ok' && \
             echo 'All services: OK'",
        ])
        .sync()
        .await?;
    eprintln!("--- All services: OK");

    eprintln!("==> All service tests passed");
    Ok(())
}

/// Create a Garage (S3-compatible) service container.
/// Create a post3 S3-compatible service container (filesystem backend, no auth).
fn s3_service(client: &dagger_sdk::Query) -> dagger_sdk::Service {
    let registry = std::env::var("CI_REGISTRY").unwrap_or_else(|_| "git.kjuulh.io".into());
    let user = std::env::var("CI_REGISTRY_USER").unwrap_or_else(|_| "kjuulh".into());

    let container = client.container();

    // Authenticate to the private registry if credentials are available.
    let container = if let Ok(password) = std::env::var("CI_REGISTRY_PASSWORD") {
        container.with_registry_auth(
            &registry,
            &user,
            client.set_secret("registry-password-s3", &password),
        )
    } else {
        container
    };

    container
        .from(&format!("{registry}/{user}/post3:20260227125609"))
        .with_exposed_port(9000)
        .as_service_opts(
            dagger_sdk::ContainerAsServiceOptsBuilder::default()
                .use_entrypoint(true)
                .args(vec![
                    "serve",
                    "--backend", "fs",
                    "--data-dir", "/tmp/post3",
                    "--host", "0.0.0.0:9000",
                ])
                .build()
                .unwrap(),
        )
}

/// Load only Rust-relevant source files from host.
/// Using include patterns prevents cache busting from unrelated file changes
/// (templates, docs, configs, etc.).
fn load_source(client: &dagger_sdk::Query) -> eyre::Result<dagger_sdk::Directory> {
    let src = client.host().directory_opts(
        ".",
        dagger_sdk::HostDirectoryOptsBuilder::default()
            .include(vec![
                "**/*.rs",
                "**/Cargo.toml",
                "Cargo.lock",
                ".sqlx/**",
                "**/*.sql",
                "**/*.zsh",
                "**/*.snap",
                "**/*.toml",
                "**/*.tmpl",
                "**/*.cue",
            ])
            .build()?,
    );
    Ok(src)
}

/// Load dependency-only source (Cargo.toml + Cargo.lock + .sqlx, no .rs or tests).
fn load_dep_source(client: &dagger_sdk::Query) -> eyre::Result<dagger_sdk::Directory> {
    let src = client.host().directory_opts(
        ".",
        dagger_sdk::HostDirectoryOptsBuilder::default()
            .include(vec!["**/Cargo.toml", "Cargo.lock", ".sqlx/**"])
            .build()?,
    );
    Ok(src)
}

/// Create skeleton source files so cargo can resolve deps without real source.
fn create_skeleton_files(client: &dagger_sdk::Query) -> eyre::Result<dagger_sdk::Directory> {
    let main_content = r#"fn main() { panic!("skeleton"); }"#;
    let lib_content = r#"pub fn _skeleton() {}"#;

    let crate_paths = discover_crates()?;
    let mut dir = client.directory();

    for crate_path in &crate_paths {
        let src_dir = crate_path.join("src");
        dir = dir.with_new_file(
            src_dir.join("main.rs").to_string_lossy().to_string(),
            main_content,
        );
        dir = dir.with_new_file(
            src_dir.join("lib.rs").to_string_lossy().to_string(),
            lib_content,
        );
    }

    // Also add skeleton for ci/ crate itself.
    dir = dir.with_new_file("ci/src/main.rs".to_string(), main_content);

    Ok(dir)
}

/// Discover workspace crate directories on the host by recursively finding Cargo.toml files.
fn discover_crates() -> eyre::Result<Vec<PathBuf>> {
    let mut crate_paths = Vec::new();
    for search_root in ["crates", "examples", "components"] {
        let root = PathBuf::from(search_root);
        if root.is_dir() {
            find_crates_recursive(&root, &mut crate_paths)?;
        }
    }
    Ok(crate_paths)
}

fn find_crates_recursive(dir: &PathBuf, out: &mut Vec<PathBuf>) -> eyre::Result<()> {
    if dir.join("Cargo.toml").exists() {
        out.push(dir.clone());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let name = entry.file_name();
            if name == "target" || name == "node_modules" {
                continue;
            }
            find_crates_recursive(&entry.path(), out)?;
        }
    }
    Ok(())
}

/// Build the base Rust container with all deps cached (native x86_64, used for check/test).
async fn build_base(client: &dagger_sdk::Query) -> eyre::Result<dagger_sdk::Container> {
    build_base_for_platform(client, &PLATFORM_AMD64).await
}

/// Build the base Rust container for a specific target platform.
async fn build_base_for_platform(
    client: &dagger_sdk::Query,
    spec: &PlatformSpec,
) -> eyre::Result<dagger_sdk::Container> {
    let src = load_source(client)?;
    let dep_src = load_dep_source(client)?;
    let skeleton = create_skeleton_files(client)?;

    let dep_src_with_skeleton = dep_src.with_directory(".", skeleton);

    // Base rust image with build tools — always x86_64 (cross-compile for arm64).
    let mut rust_base = client
        .container()
        .from("rust:1.93-trixie")
        .with_exec(vec!["apt", "update"])
        .with_exec(vec!["apt", "install", "-y", "clang", "wget", "git"])
        // Git config needed for tests that commit.
        .with_exec(vec!["git", "config", "--global", "user.email", "ci@forest.dev"])
        .with_exec(vec!["git", "config", "--global", "user.name", "Forest CI"])
        .with_exec(vec!["git", "config", "--global", "init.defaultBranch", "main"]);

    if spec.is_cross {
        // Cross-compilation: install cross-linker + add rustup target.
        // Mold doesn't support cross-linking, so we use the platform's gcc linker directly.
        if !spec.extra_apt_pkgs.is_empty() {
            let mut apt_cmd = vec!["apt", "install", "-y"];
            apt_cmd.extend(spec.extra_apt_pkgs.iter().copied());
            rust_base = rust_base.with_exec(apt_cmd);
        }
        rust_base = rust_base
            .with_exec(vec!["rustup", "target", "add", spec.rust_target]);
        for &(key, val) in spec.extra_env {
            rust_base = rust_base.with_env_variable(key, val);
        }
    } else {
        // Native build: install mold linker for faster linking.
        rust_base = rust_base
            .with_exec(vec![
                "wget",
                "-q",
                &format!(
                    "https://github.com/rui314/mold/releases/download/v{MOLD_VERSION}/mold-{MOLD_VERSION}-x86_64-linux.tar.gz"
                ),
            ])
            .with_exec(vec![
                "tar",
                "-xf",
                &format!("mold-{MOLD_VERSION}-x86_64-linux.tar.gz"),
            ])
            .with_exec(vec![
                "mv",
                &format!("mold-{MOLD_VERSION}-x86_64-linux/bin/mold"),
                "/usr/bin/mold",
            ]);
    }

    // Install sccache.
    rust_base = rust_base
        .with_exec(vec![
            "wget",
            "-q",
            &format!(
                "https://github.com/mozilla/sccache/releases/download/v{SCCACHE_VERSION}/sccache-v{SCCACHE_VERSION}-x86_64-unknown-linux-musl.tar.gz"
            ),
        ])
        .with_exec(vec![
            "tar",
            "-xf",
            &format!("sccache-v{SCCACHE_VERSION}-x86_64-unknown-linux-musl.tar.gz"),
        ])
        .with_exec(vec![
            "mv",
            &format!("sccache-v{SCCACHE_VERSION}-x86_64-unknown-linux-musl/sccache"),
            "/usr/bin/sccache",
        ])
        .with_env_variable("RUSTC_WRAPPER", "sccache")
        .with_env_variable("SCCACHE_MEMCACHED_ENDPOINT", SCCACHE_MEMCACHED_ENDPOINT);

    let target_args: Vec<&str> = vec!["--target", spec.rust_target];

    // Step 1: build deps with skeleton source (cacheable layer).
    let mut prebuild_cmd = vec!["cargo", "build", "--release", "--bin", BIN_NAME];
    prebuild_cmd.extend_from_slice(&target_args);

    let prebuild = rust_base
        .clone()
        .with_workdir("/mnt/src")
        .with_env_variable("SQLX_OFFLINE", "true")
        .with_directory("/mnt/src", dep_src_with_skeleton)
        .with_exec(prebuild_cmd);

    // Step 2: copy cargo registry from prebuild (avoids re-downloading deps).
    let build_container = rust_base
        .with_workdir("/mnt/src")
        .with_env_variable("SQLX_OFFLINE", "true")
        .with_directory("/usr/local/cargo", prebuild.directory("/usr/local/cargo"))
        .with_directory("/mnt/src/", src);

    Ok(build_container)
}

/// Create a Postgres service container.
fn postgres_service(client: &dagger_sdk::Query) -> dagger_sdk::Service {
    client
        .container()
        .from("postgres:17")
        .with_env_variable("POSTGRES_USER", "forest")
        .with_env_variable("POSTGRES_PASSWORD", "forest")
        .with_env_variable("POSTGRES_DB", "forest")
        .with_exposed_port(5432)
        .as_service()
}

/// Create a NATS service container.
fn nats_service(client: &dagger_sdk::Query) -> dagger_sdk::Service {
    client
        .container()
        .from("nats:2")
        .with_exposed_port(4222)
        .as_service()
}

/// Return the base container with live Postgres + NATS + S3 for runtime tests.
/// Compilation uses SQLX_OFFLINE=true (the checked-in .sqlx/ cache).
/// The live services are needed for integration tests at runtime.
fn with_services(
    client: &dagger_sdk::Query,
    base: &dagger_sdk::Container,
) -> dagger_sdk::Container {
    let pg = postgres_service(client);
    let nats = nats_service(client);
    let s3 = s3_service(client);

    base.clone()
        .with_service_binding("postgres", pg)
        .with_service_binding("nats", nats)
        .with_service_binding("s3", s3)
        .with_env_variable(
            "DATABASE_URL",
            "postgres://forest:forest@postgres:5432/forest",
        )
        .with_env_variable("NATS_URL", "nats://nats:4222")
        .with_env_variable("S3_ENDPOINT", "http://s3:9000")
        .with_env_variable("S3_BUCKET", "forest")
        .with_env_variable("S3_ACCESS_KEY", "test")
        .with_env_variable("S3_SECRET_KEY", "test")
        // Create the S3 bucket before tests run.
        .with_exec(vec![
            "sh", "-c",
            "until curl -sf http://s3:9000/ > /dev/null 2>&1; do sleep 1; done && \
             curl -sf -X PUT http://s3:9000/forest || true",
        ])
}

/// Build release binary and package into a slim image for a specific platform.
async fn build_release_image_for_platform(
    client: &dagger_sdk::Query,
    base: &dagger_sdk::Container,
    spec: &PlatformSpec,
) -> eyre::Result<dagger_sdk::Container> {
    let mut build_cmd = vec!["cargo", "build", "--release", "--bin", BIN_NAME];
    build_cmd.extend_from_slice(&["--target", spec.rust_target]);

    let built = base.clone().with_exec(build_cmd);

    let binary = built.file(format!(
        "/mnt/src/target/{}/release/{BIN_NAME}",
        spec.rust_target,
    ));

    // Use a platform-specific base image matching the target arch.
    let final_image = client
        .container_opts(
            dagger_sdk::QueryContainerOptsBuilder::default()
                .platform(dagger_sdk::Platform(spec.platform.to_string()))
                .build()?,
        )
        .from("debian:13-slim")
        .with_exec(vec!["apt", "update"])
        .with_exec(vec![
            "apt",
            "install",
            "-y",
            "--no-install-recommends",
            "git",
            "ca-certificates",
        ])
        .with_file(format!("/usr/local/bin/{BIN_NAME}"), binary)
        .with_entrypoint(vec![BIN_NAME]);

    eprintln!("--- release image built for {}", spec.platform);
    Ok(final_image)
}

/// Build release images for both amd64 and arm64.
async fn build_release_images(
    client: &dagger_sdk::Query,
) -> eyre::Result<(dagger_sdk::Container, dagger_sdk::Container)> {
    eprintln!("--- building amd64 release image");
    let base_amd64 = build_base_for_platform(client, &PLATFORM_AMD64).await?;
    let image_amd64 = build_release_image_for_platform(client, &base_amd64, &PLATFORM_AMD64).await?;

    eprintln!("--- building arm64 release image (cross-compile)");
    let base_arm64 = build_base_for_platform(client, &PLATFORM_ARM64).await?;
    let image_arm64 = build_release_image_for_platform(client, &base_arm64, &PLATFORM_ARM64).await?;

    Ok((image_amd64, image_arm64))
}

/// Publish multi-platform image to container registry with latest, commit, and timestamp tags.
async fn publish_image(
    client: &dagger_sdk::Query,
    image_amd64: &dagger_sdk::Container,
    image_arm64: &dagger_sdk::Container,
) -> eyre::Result<()> {
    let registry = std::env::var("CI_REGISTRY").unwrap_or_else(|_| "git.kjuulh.io".into());
    let user = std::env::var("CI_REGISTRY_USER").unwrap_or_else(|_| "kjuulh".into());
    let image_name =
        std::env::var("CI_IMAGE_NAME").unwrap_or_else(|_| format!("{registry}/{user}/forest"));

    let password = std::env::var("CI_REGISTRY_PASSWORD")
        .map_err(|_| eyre::eyre!("CI_REGISTRY_PASSWORD must be set for publishing"))?;

    let commit = git_short_hash()?;
    let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();

    let tags = vec!["latest".to_string(), commit, timestamp];

    let secret = client.set_secret("registry-password", &password);

    let authed_amd64 = image_amd64
        .clone()
        .with_registry_auth(&registry, &user, secret.clone());
    let authed_arm64 = image_arm64
        .clone()
        .with_registry_auth(&registry, &user, secret);

    // Get the arm64 container ID to pass as platform variant.
    let arm64_id = authed_arm64.id().await?;

    for tag in &tags {
        let image_ref = format!("{image_name}:{tag}");
        authed_amd64
            .publish_opts(
                &image_ref,
                dagger_sdk::ContainerPublishOptsBuilder::default()
                    .platform_variants(vec![arm64_id.clone()])
                    .build()?,
            )
            .await?;
        eprintln!("--- published {image_ref} (linux/amd64 + linux/arm64)");
    }

    Ok(())
}

/// Query the memcached exporter and print cache-relevant metrics.
async fn print_memcached_metrics(client: &dagger_sdk::Query, label: &str) -> eyre::Result<()> {
    eprintln!("--- memcached metrics ({label})");
    let output = client
        .container()
        .from("alpine:3")
        .with_exec(vec!["wget", "-qO-", MEMCACHED_METRICS_URL])
        .stdout()
        .await?;
    for line in output.lines() {
        if line.starts_with('#') {
            continue;
        }
        if line.contains("cmd_get")
            || line.contains("cmd_set")
            || line.contains("get_hits")
            || line.contains("get_misses")
            || line.contains("curr_items")
            || line.contains("bytes")
        {
            eprintln!("  {line}");
        }
    }
    Ok(())
}

/// Wrap a cargo command so sccache stats are printed in the same exec
/// (the sccache server only lives for the duration of the process tree).
fn cargo_with_stats(base: &dagger_sdk::Container, cargo_args: &str) -> dagger_sdk::Container {
    base.clone().with_exec(vec![
        "sh",
        "-c",
        &format!("{cargo_args} && sccache --show-stats"),
    ])
}

/// Get the short git commit hash from the host.
fn git_short_hash() -> eyre::Result<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()?;
    let hash = String::from_utf8(output.stdout)?.trim().to_string();
    if hash.is_empty() {
        return Err(eyre::eyre!("could not determine git commit hash"));
    }
    Ok(hash)
}
