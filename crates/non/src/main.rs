use tracing_subscriber::EnvFilter;

mod cli;
mod grpc;
mod services;
mod state;

mod component_cache;
mod user_config;
mod user_locations;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("notmad=warn".parse().unwrap()),
        )
        .init();

    cli::execute().await?;

    Ok(())
}
