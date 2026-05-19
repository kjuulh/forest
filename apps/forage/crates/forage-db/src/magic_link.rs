use chrono::{DateTime, Utc};
use forage_core::auth::magic_link::{MagicLinkError, MagicLinkStore};
use sqlx::PgPool;

pub struct PgMagicLinkStore {
    pool: PgPool,
}

impl PgMagicLinkStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl MagicLinkStore for PgMagicLinkStore {
    async fn store_token(
        &self,
        token_type: &str,
        token_hash: &str,
        email: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<(), MagicLinkError> {
        sqlx::query(
            "INSERT INTO magic_link_tokens (token_hash, token_type, email, expires_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(token_hash)
        .bind(token_type)
        .bind(email)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .map_err(|e| MagicLinkError::Store(e.to_string()))?;
        Ok(())
    }

    async fn verify_and_consume(
        &self,
        token_type: &str,
        token_hash: &str,
    ) -> Result<Option<String>, MagicLinkError> {
        // Atomic: delete the row and return its email if not expired and
        // the token_type matches. Cross-type redemption is impossible.
        let row: Option<(String,)> = sqlx::query_as(
            "DELETE FROM magic_link_tokens \
             WHERE token_hash = $1 AND token_type = $2 AND expires_at > NOW() \
             RETURNING email",
        )
        .bind(token_hash)
        .bind(token_type)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MagicLinkError::Store(e.to_string()))?;
        Ok(row.map(|r| r.0))
    }

    async fn count_recent(
        &self,
        token_type: &str,
        email: &str,
        since: DateTime<Utc>,
    ) -> Result<u64, MagicLinkError> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM magic_link_tokens \
             WHERE token_type = $1 AND email = $2 AND created_at >= $3",
        )
        .bind(token_type)
        .bind(email)
        .bind(since)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| MagicLinkError::Store(e.to_string()))?;
        Ok(row.0 as u64)
    }

    async fn reap_expired(&self) -> Result<u64, MagicLinkError> {
        let result = sqlx::query("DELETE FROM magic_link_tokens WHERE expires_at < NOW()")
            .execute(&self.pool)
            .await
            .map_err(|e| MagicLinkError::Store(e.to_string()))?;
        Ok(result.rows_affected())
    }
}
