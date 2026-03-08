use anyhow::Context;
use drop_queue::DropQueue;
use sqlx::PgPool;

#[derive(Clone)]
pub struct State {
    pub db: PgPool,
    pub nats: async_nats::Client,
    pub drop_queue: DropQueue,

    pub config: Config,
}

#[derive(Clone)]
pub struct Config {
    pub external_host: String,
    pub terraform_external_host: String,
    pub password_secret_key: String,
    pub access_token_secret_key: Vec<u8>,
    pub refresh_token_secret_key: Vec<u8>,
}

impl State {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let pool = sqlx::PgPool::connect(
            &std::env::var("DATABASE_URL").context("failed to find DATABASE_URL in env")?,
        )
        .await?;

        // TODO: As we cannot lock with cockroach, we should consider sending it to a migration queue instead
        sqlx::migrate!("./migrations/")
            .set_locking(false)
            .run(&pool)
            .await?;

        let nats_url =
            std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
        let nats = async_nats::connect(&nats_url)
            .await
            .context("failed to connect to NATS")?;

        Ok(Self {
            db: pool,
            nats,
            drop_queue: DropQueue::new(),
            config,
        })
    }
}
