use tracing_subscriber::EnvFilter;

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
            tracing_subscriber::fmt()
                .pretty()
                .with_env_filter(
                    EnvFilter::from_default_env().add_directive("notmad=trace".parse()?),
                )
                .init();
        }
    }

    forest_server::cli::execute().await?;

    Ok(())
}
