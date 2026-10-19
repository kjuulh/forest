#![allow(dead_code, clippy::too_many_arguments)]

mod cli;
mod download;
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
    let _ = rustls::crypto::ring::default_provider().install_default();

    dotenvy::dotenv().ok();

    if let Err(err) = cli::execute().await {
        eprintln!("{err:?}");
        return std::process::ExitCode::FAILURE;
    }

    std::process::ExitCode::SUCCESS
}
