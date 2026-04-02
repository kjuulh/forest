use anyhow::Context;
use drop_queue::DropQueue;
use forest_event_store::EventStore;
use sqlx::PgPool;

/// Connect to NATS with optional authentication.
///
/// Supported auth methods (checked in order):
/// - **Credentials file**: Set `NATS_CREDS` to the full creds file content (JWT + NKey seed).
/// - **NKey**: Set `NATS_NKEY_SEED` to the seed (starts with `SU`).
/// - **User/Password**: Set `NATS_USER` and `NATS_PASSWORD`.
/// - **Token**: Set `NATS_TOKEN`.
/// - **None**: Plain unauthenticated connection.
async fn connect_nats(url: &str) -> anyhow::Result<async_nats::Client> {
    if let Ok(creds) = std::env::var("NATS_CREDS") {
        tracing::info!("connecting to NATS with credentials file");
        let client = async_nats::ConnectOptions::with_credentials(&creds)
            .context("failed to parse NATS_CREDS")?
            .connect(url)
            .await?;
        return Ok(client);
    }

    if let Ok(seed) = std::env::var("NATS_NKEY_SEED") {
        tracing::info!("connecting to NATS with NKey auth");
        let client = async_nats::ConnectOptions::with_nkey(seed)
            .connect(url)
            .await?;
        return Ok(client);
    }

    if let (Ok(user), Ok(password)) = (
        std::env::var("NATS_USER"),
        std::env::var("NATS_PASSWORD"),
    ) {
        tracing::info!("connecting to NATS with user/password auth");
        let client = async_nats::ConnectOptions::with_user_and_password(user, password)
            .connect(url)
            .await?;
        return Ok(client);
    }

    if let Ok(token) = std::env::var("NATS_TOKEN") {
        tracing::info!("connecting to NATS with token auth");
        let client = async_nats::ConnectOptions::with_token(token)
            .connect(url)
            .await?;
        return Ok(client);
    }

    tracing::info!("connecting to NATS without auth");
    Ok(async_nats::connect(url).await?)
}

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
        let nats = connect_nats(&nats_url)
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
