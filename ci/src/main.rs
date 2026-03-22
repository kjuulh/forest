use std::path::PathBuf;

use clap::Parser;

const BIN_NAME: &str = "forest-server";
const MOLD_VERSION: &str = "2.40.4";

#[derive(Parser)]
#[command(name = "ci")]
enum Cli {
    /// Run PR validation pipeline (check + test + build)
    Pr,
    /// Run main branch pipeline (check + test + build + publish)
    Main,
    /// Build forest CLI release snapshot (dry run, no publish)
    ReleaseSnapshot,
    /// Build and publish forest CLI release via GoReleaser
    Release,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Cli::parse();

    dagger_sdk::connect(|client| async move {
        match cli {
            Cli::Pr => run_pr(&client).await?,
            Cli::Main => run_main(&client).await?,
            Cli::ReleaseSnapshot => run_goreleaser(&client, false).await?,
            Cli::Release => run_goreleaser(&client, true).await?,
        }
        Ok(())
    })
    .await?;

    Ok(())
}

async fn run_pr(client: &dagger_sdk::Query) -> eyre::Result<()> {
    eprintln!("==> PR pipeline: check + test + build");

    let base = build_base(client).await?;

    eprintln!("--- cargo check --workspace");
    base.clone()
        .with_exec(vec!["cargo", "check", "--workspace"])
        .sync()
        .await?;

    eprintln!("--- running tests");
    with_services(client, &base)
        .with_exec(vec![
            "cargo",
            "test",
            "--workspace",
            "--exclude",
            "forest-event-store",
        ])
        .sync()
        .await?;

    eprintln!("--- building release image");
    let _image = build_release_image(client, &base).await?;

    eprintln!("==> PR pipeline complete");
    Ok(())
}

async fn run_main(client: &dagger_sdk::Query) -> eyre::Result<()> {
    eprintln!("==> Main pipeline: check + test + build + publish");

    let base = build_base(client).await?;

    eprintln!("--- cargo check --workspace");
    base.clone()
        .with_exec(vec!["cargo", "check", "--workspace"])
        .sync()
        .await?;

    eprintln!("--- running tests");
    with_services(client, &base)
        .with_exec(vec![
            "cargo",
            "test",
            "--workspace",
            "--exclude",
            "forest-event-store",
        ])
        .sync()
        .await?;

    eprintln!("--- building release image");
    let image = build_release_image(client, &base).await?;

    eprintln!("--- publishing image");
    publish_image(client, &image).await?;

    eprintln!("==> Main pipeline complete");
    Ok(())
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
    for search_root in ["crates", "examples"] {
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

/// Build the base Rust container with all deps cached.
async fn build_base(client: &dagger_sdk::Query) -> eyre::Result<dagger_sdk::Container> {
    let src = load_source(client)?;
    let dep_src = load_dep_source(client)?;
    let skeleton = create_skeleton_files(client)?;

    let dep_src_with_skeleton = dep_src.with_directory(".", skeleton);

    // Base rust image with build tools.
    let rust_base = client
        .container()
        .from("rust:1.93-trixie")
        .with_exec(vec!["apt", "update"])
        .with_exec(vec!["apt", "install", "-y", "clang", "wget", "git"])
        // Git config needed for tests that commit.
        .with_exec(vec!["git", "config", "--global", "user.email", "ci@forest.dev"])
        .with_exec(vec!["git", "config", "--global", "user.name", "Forest CI"])
        .with_exec(vec!["git", "config", "--global", "init.defaultBranch", "main"])
        // Install mold linker.
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

    // Step 1: build deps with skeleton source (cacheable layer).
    // SQLX_OFFLINE=true uses the checked-in .sqlx/ query cache instead of a live database.
    let prebuild = rust_base
        .clone()
        .with_workdir("/mnt/src")
        .with_env_variable("SQLX_OFFLINE", "true")
        .with_directory("/mnt/src", dep_src_with_skeleton)
        .with_exec(vec!["cargo", "build", "--release", "--bin", BIN_NAME]);

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

/// Return the base container with live Postgres + NATS for runtime tests.
/// Compilation uses SQLX_OFFLINE=true (the checked-in .sqlx/ cache).
/// The live services are needed for integration tests at runtime.
fn with_services(
    client: &dagger_sdk::Query,
    base: &dagger_sdk::Container,
) -> dagger_sdk::Container {
    let pg = postgres_service(client);
    let nats = nats_service(client);

    base.clone()
        .with_service_binding("postgres", pg)
        .with_service_binding("nats", nats)
        .with_env_variable(
            "DATABASE_URL",
            "postgres://forest:forest@postgres:5432/forest",
        )
        .with_env_variable("NATS_URL", "nats://nats:4222")
}

/// Build release binary and package into a slim image.
async fn build_release_image(
    client: &dagger_sdk::Query,
    base: &dagger_sdk::Container,
) -> eyre::Result<dagger_sdk::Container> {
    let built = base
        .clone()
        .with_exec(vec!["cargo", "build", "--release", "--bin", BIN_NAME]);

    let binary = built.file(format!("/mnt/src/target/release/{BIN_NAME}"));

    // Distroless cc-debian13 matches the build image's glibc (trixie/2.38+)
    // and includes libgcc + ca-certificates with no shell or package manager.
    let final_image = client
        .container()
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
        .with_exec(vec![BIN_NAME, "--help"]);

    final_image.sync().await?;

    // Set the final entrypoint for the published image.
    let final_image = final_image.with_entrypoint(vec![BIN_NAME]);

    eprintln!("--- release image built successfully");
    Ok(final_image)
}

/// Publish image to container registry with latest, commit, and timestamp tags.
async fn publish_image(
    client: &dagger_sdk::Query,
    image: &dagger_sdk::Container,
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

    let authed = image.clone().with_registry_auth(
        &registry,
        &user,
        client.set_secret("registry-password", &password),
    );

    for tag in &tags {
        let image_ref = format!("{image_name}:{tag}");
        authed
            .publish_opts(
                &image_ref,
                dagger_sdk::ContainerPublishOptsBuilder::default().build()?,
            )
            .await?;
        eprintln!("--- published {image_ref}");
    }

    Ok(())
}

/// Build the goreleaser container (mirrors Dockerfile.release) and run goreleaser.
/// When `publish` is true, runs `mise run release` (requires GITEA_TOKEN).
/// When false, runs `mise run release-snapshot` (local dry run).
async fn run_goreleaser(client: &dagger_sdk::Query, publish: bool) -> eyre::Result<()> {
    let task = if publish {
        "release"
    } else {
        "release-snapshot"
    };
    eprintln!("==> GoReleaser pipeline: {task}");

    // Load the full repo (goreleaser needs git history for changelog/tags).
    let src = client.host().directory_opts(
        ".",
        dagger_sdk::HostDirectoryOptsBuilder::default()
            .exclude(vec!["target", "dist", "node_modules"])
            .build()?,
    );

    // Build the release container: debian + mise (rust, goreleaser, zig, cargo-zigbuild).
    let container = client
        .container()
        .from("debian:trixie-slim")
        .with_exec(vec!["apt-get", "update"])
        .with_exec(vec![
            "apt-get",
            "install",
            "-y",
            "--no-install-recommends",
            "ca-certificates",
            "curl",
            "git",
            "build-essential",
        ])
        .with_exec(vec![
            "sh",
            "-c",
            "curl https://mise.run | MISE_INSTALL_PATH=/usr/local/bin/mise sh",
        ])
        .with_env_variable("MISE_YES", "1")
        .with_env_variable("MISE_TRUSTED_CONFIG_PATHS", "/build")
        .with_workdir("/build")
        // Copy mise.toml first and install tools (cacheable layer).
        .with_file("/build/mise.toml", src.file("mise.toml"))
        .with_exec(vec!["mise", "trust"])
        .with_exec(vec!["mise", "install"])
        // Now copy the full source.
        .with_directory("/build", src);

    // Pass secrets for publishing.
    let container = if publish {
        let token = std::env::var("GITEA_TOKEN")
            .or_else(|_| std::env::var("CI_REGISTRY_PASSWORD"))
            .map_err(|_| {
                eyre::eyre!("GITEA_TOKEN or CI_REGISTRY_PASSWORD must be set for release")
            })?;

        container
            .with_secret_variable("GITEA_TOKEN", client.set_secret("gitea-token", &token))
            .with_secret_variable("RELEASE_TOKEN", client.set_secret("release-token", &token))
    } else {
        container
    };

    container
        .with_exec(vec!["mise", "run", task])
        .sync()
        .await?;

    eprintln!("==> GoReleaser {task} complete");
    Ok(())
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
