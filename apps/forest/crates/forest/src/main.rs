#![allow(dead_code, clippy::too_many_arguments)]
use tracing_subscriber::EnvFilter;

mod cli;
mod grpc;
mod services;
mod state;

mod component_registry;

mod component_cache;
mod requirements;
mod user_config;
mod user_locations;
mod user_state;

mod contexts;
mod contracts;
mod diagnostics;
mod features;
mod global;
mod lockfile;
mod version_spec;
mod models;
mod project_artifacts;

mod otel;
mod tools;

mod forest_context;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    dotenvy::dotenv().ok();

    // All tracing output (DEBUG/INFO/WARN/ERROR) goes to stderr so it never
    // contaminates command stdout — `forest X --format json | jq` stays clean,
    // and `slug=$(forest project publish)` doesn't capture log lines.
    tracing_subscriber::fmt()
        .pretty()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("notmad=warn".parse().unwrap()),
        )
        .init();

    // Print errors ourselves (Debug form, same as anyhow's default Termination)
    // rather than returning a Result from `main`. The build/publish path
    // (DATA-312) renders miette diagnostics into the error string at the leaf;
    // letting Rust's default `Error: {:?}` Termination prefix glue itself onto
    // a multi-line graphical report would mangle it. A plain `eprintln!` keeps
    // both pre-rendered reports and ordinary anyhow chains intact.
    if let Err(err) = cli::execute().await {
        eprintln!("{err:?}");
        return std::process::ExitCode::FAILURE;
    }

    std::process::ExitCode::SUCCESS
}
