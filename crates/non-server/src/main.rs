#![allow(dead_code)]

mod cli;
mod repositories;
mod services;
mod state;
pub use state::*;

mod destination_services;
mod destinations;

mod grpc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    cli::execute().await?;

    Ok(())
}
