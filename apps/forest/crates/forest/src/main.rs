#![allow(dead_code, clippy::too_many_arguments)]

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
mod models;
mod project_artifacts;
mod version_spec;

mod otel;
mod tools;

mod forest_context;
mod ui;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    dotenvy::dotenv().ok();

    // tracing is initialised inside `cli::execute` once args are parsed, so it
    // can honour `--verbose` and the interactive-vs-CI audience (see
    // `ui::init_logging`). All tracing output goes to stderr; stdout stays
    // clean for machine output (`--format`).

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
