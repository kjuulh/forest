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
    let log_level = std::env::var("LOG_LEVEL");

    match log_level.as_ref().map(|r| r.as_str()) {
        Ok("json") => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(
                    EnvFilter::from_default_env().add_directive("notmad=trace".parse()?),
                )
                .init();
        }
        Ok("short") => {
            tracing_subscriber::fmt()
                .with_line_number(false)
                .with_target(false)
                .with_file(false)
                .with_level(true)
                .with_env_filter(
                    EnvFilter::from_default_env().add_directive("notmad=trace".parse()?),
                )
                .init();
        }
        _ => {
            // default to pretty logging
            tracing_subscriber::fmt()
                .pretty()
                .with_env_filter(
                    EnvFilter::from_default_env().add_directive("notmad=trace".parse()?),
                )
                .init();
        }
    }

    cli::execute().await?;

    Ok(())
}
