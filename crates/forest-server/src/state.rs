use anyhow::Context;
use drop_queue::DropQueue;
use sqlx::PgPool;

#[derive(Clone)]
pub struct State {
    pub db: PgPool,
    pub drop_queue: DropQueue,

    pub config: Config,
}

#[derive(Clone)]
pub struct Config {
    pub external_host: Option<String>,
    pub terraform_external_host: Option<String>,
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

        Ok(Self {
            db: pool,
            drop_queue: DropQueue::new(),
            config,
        })
    }
}
