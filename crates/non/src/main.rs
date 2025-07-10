mod cli;
mod grpc;
mod services;
mod state;

mod component_cache;
mod user_config;
mod user_locations;

mod drop_queue;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    cli::execute().await?;

    Ok(())
}
