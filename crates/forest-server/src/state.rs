use anyhow::Context;
use drop_queue::DropQueue;
use forest_event_store::EventStore;
use sqlx::PgPool;

#[derive(Clone)]
pub struct State {
    pub db: PgPool,
    pub nats: async_nats::Client,
    pub drop_queue: DropQueue,
    pub event_store: EventStore,
    pub object_store: crate::object_store::ObjectStore,

    pub config: Config,
}

#[derive(Clone)]
pub struct Config {
    pub external_host: String,
    pub terraform_external_host: String,
    pub password_secret_key: String,
    pub access_token_secret_key: Vec<u8>,
    pub refresh_token_secret_key: Vec<u8>,

    /// Optional pre-hashed service account API key (SHA-256).
    /// Set via `FOREST_SERVICE_ACCOUNT_API_KEY` env var.
    /// Grants `Actor::ServiceAccount` with full cross-org access.
    pub service_account_token_hash: Option<Vec<u8>>,
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

        let event_store = EventStore::new(pool.clone());
        event_store
            .migrate()
            .await
            .context("event store migration")?;

        let nats_url =
            std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
        let nats = async_nats::connect(&nats_url)
            .await
            .context("failed to connect to NATS")?;

        let object_store = crate::object_store::ObjectStore::from_env()
            .context("failed to initialize S3 object store")?;

        Ok(Self {
            db: pool,
            nats,
            drop_queue: DropQueue::new(),
            event_store,
            object_store,
            config,
        })
    }
}
