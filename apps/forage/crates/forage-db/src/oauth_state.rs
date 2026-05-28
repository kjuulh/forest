use chrono::{DateTime, Utc};
use forage_core::auth::oauth_state::{OAuthFlowState, OAuthStateError, OAuthStateStore};
use sqlx::PgPool;

pub struct PgOAuthStateStore {
    pool: PgPool,
}

impl PgOAuthStateStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl OAuthStateStore for PgOAuthStateStore {
    async fn create(
        &self,
        provider: &str,
        state: &str,
        return_to: Option<&str>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), OAuthStateError> {
        sqlx::query(
            "INSERT INTO oauth_flow_state (provider, state, return_to, expires_at) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(provider)
        .bind(state)
        .bind(return_to)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .map_err(|e| OAuthStateError::Store(e.to_string()))?;
        Ok(())
    }

    async fn consume(
        &self,
        provider: &str,
        state: &str,
    ) -> Result<Option<OAuthFlowState>, OAuthStateError> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "DELETE FROM oauth_flow_state \
             WHERE provider = $1 AND state = $2 AND expires_at > NOW() \
             RETURNING return_to",
        )
        .bind(provider)
        .bind(state)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| OAuthStateError::Store(e.to_string()))?;
        Ok(row.map(|(return_to,)| OAuthFlowState { return_to }))
    }

    async fn reap_expired(&self) -> Result<u64, OAuthStateError> {
        let result = sqlx::query("DELETE FROM oauth_flow_state WHERE expires_at < NOW()")
            .execute(&self.pool)
            .await
            .map_err(|e| OAuthStateError::Store(e.to_string()))?;
        Ok(result.rows_affected())
    }
}
