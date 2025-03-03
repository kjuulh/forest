use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

pub mod cli;
pub mod model;
pub mod plan_reconciler;
pub mod script;
pub mod state;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::WARN.into())
                .with_env_var("FOREST_LOG_LEVEL")
                .from_env()?,
        )
        .init();

    cli::execute().await?;

    Ok(())
}
