pub mod cli;
pub mod model;
pub mod plan_reconciler;
pub mod script;
pub mod state;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    cli::execute().await?;

    Ok(())
}
