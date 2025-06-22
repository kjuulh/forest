use anyhow::Context;
use sqlx::PgPool;

#[derive(Clone)]
pub struct State {
    pub db: PgPool,
}

impl State {
    pub async fn new() -> anyhow::Result<Self> {
        let pool = sqlx::PgPool::connect(
            &std::env::var("DATABASE_URL").context("failed to find DATABASE_URL in env")?,
        )
        .await?;

        // TODO: As we cannot lock with cockroach, we should consider sending it to a migration queue instead
        sqlx::migrate!("./migrations/")
            .set_locking(false)
            .run(&pool)
            .await?;

        Ok(Self { db: pool })
    }
}
