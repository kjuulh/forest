#![allow(dead_code)]

// TODO: we should select only the destinations specified in the project

mod cli;
mod repositories;
mod services;
mod state;
pub use state::*;
use tracing_subscriber::EnvFilter;

mod destination_services;
mod destinations;

mod grpc;
mod scheduler;
mod temp_dir;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("notmad=trace".parse()?))
        .init();

    cli::execute().await?;

    Ok(())
}
