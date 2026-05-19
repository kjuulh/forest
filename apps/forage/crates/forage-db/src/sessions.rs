use std::time::Duration;

use chrono::{DateTime, Utc};
use forage_core::auth::UserEmail;
use forage_core::session::{CachedOrg, CachedUser, SessionData, SessionError, SessionId, SessionStore};
use moka::future::Cache;
use sqlx::PgPool;

/// PostgreSQL-backed session store with a Moka write-through cache.
/// Reads check the cache first, falling back to Postgres on miss.
/// Writes update both cache and Postgres atomically.
pub struct PgSessionStore {
    pool: PgPool,
    cache: Cache<String, SessionData>,
}

impl PgSessionStore {
    pub fn new(pool: PgPool) -> Self {
        let cache = Cache::builder()
            .max_capacity(10_000)
            .time_to_idle(Duration::from_secs(30 * 60)) // evict after 30min idle
            .build();
        Self { pool, cache }
    }

    /// Remove sessions inactive for longer than `max_inactive_days`.
    pub async fn reap_expired(&self, max_inactive_days: i64) -> Result<u64, SessionError> {
        let cutoff = Utc::now() - chrono::Duration::days(max_inactive_days);
        let result = sqlx::query("DELETE FROM sessions WHERE last_seen_at < $1")
            .bind(cutoff)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::Store(e.to_string()))?;

        // Moka handles its own TTL eviction, but force a sync for reaped sessions
        self.cache.run_pending_tasks().await;

        Ok(result.rows_affected())
    }
}

#[async_trait::async_trait]
impl SessionStore for PgSessionStore {
    async fn create(&self, data: SessionData) -> Result<SessionId, SessionError> {
        let id = SessionId::generate();
        let (user_id, username, emails_json, orgs_json) = extract_user_fields(&data)?;

        sqlx::query(
            "INSERT INTO sessions (session_id, access_token, refresh_token, access_expires_at, user_id, username, user_emails, user_orgs, csrf_token, created_at, last_seen_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(id.as_str())
        .bind(&data.access_token)
        .bind(&data.refresh_token)
        .bind(data.access_expires_at)
        .bind(&user_id)
        .bind(&username)
        .bind(&emails_json)
        .bind(&orgs_json)
        .bind(&data.csrf_token)
        .bind(data.created_at)
        .bind(data.last_seen_at)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::Store(e.to_string()))?;

        // Populate cache
        self.cache.insert(id.as_str().to_string(), data).await;

        Ok(id)
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionData>, SessionError> {
        // Check cache first
        if let Some(data) = self.cache.get(id.as_str()).await {
            return Ok(Some(data));
        }

        // Cache miss — fall back to Postgres
        let row: Option<SessionRow> = sqlx::query_as(
            "SELECT access_token, refresh_token, access_expires_at, user_id, username, user_emails, user_orgs, csrf_token, created_at, last_seen_at
             FROM sessions WHERE session_id = $1",
        )
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| SessionError::Store(e.to_string()))?;

        if let Some(row) = row {
            let data = row.into_session_data();
            // Backfill cache
            self.cache.insert(id.as_str().to_string(), data.clone()).await;
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }

    async fn update(&self, id: &SessionId, data: SessionData) -> Result<(), SessionError> {
        let (user_id, username, emails_json, orgs_json) = extract_user_fields(&data)?;

        sqlx::query(
            "UPDATE sessions SET access_token = $1, refresh_token = $2, access_expires_at = $3, user_id = $4, username = $5, user_emails = $6, user_orgs = $7, csrf_token = $8, last_seen_at = $9
             WHERE session_id = $10",
        )
        .bind(&data.access_token)
        .bind(&data.refresh_token)
        .bind(data.access_expires_at)
        .bind(&user_id)
        .bind(&username)
        .bind(&emails_json)
        .bind(&orgs_json)
        .bind(&data.csrf_token)
        .bind(data.last_seen_at)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::Store(e.to_string()))?;

        // Update cache
        self.cache.insert(id.as_str().to_string(), data).await;

        Ok(())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), SessionError> {
        sqlx::query("DELETE FROM sessions WHERE session_id = $1")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::Store(e.to_string()))?;

        // Evict from cache
        self.cache.invalidate(id.as_str()).await;

        Ok(())
    }
}

/// Extract user fields for SQL binding, shared by create and update.
fn extract_user_fields(
    data: &SessionData,
) -> Result<
    (
        Option<String>,
        Option<String>,
        Option<serde_json::Value>,
        Option<serde_json::Value>,
    ),
    SessionError,
> {
    match &data.user {
        Some(u) => Ok((
            Some(u.user_id.clone()),
            Some(u.username.clone()),
            Some(
                serde_json::to_value(&u.emails)
                    .map_err(|e| SessionError::Store(e.to_string()))?,
            ),
            Some(
                serde_json::to_value(&u.orgs)
                    .map_err(|e| SessionError::Store(e.to_string()))?,
            ),
        )),
        None => Ok((None, None, None, None)),
    }
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    access_token: String,
    refresh_token: String,
    access_expires_at: DateTime<Utc>,
    user_id: Option<String>,
    username: Option<String>,
    user_emails: Option<serde_json::Value>,
    user_orgs: Option<serde_json::Value>,
    csrf_token: String,
    created_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
}

impl SessionRow {
    fn into_session_data(self) -> SessionData {
        let user = match (self.user_id, self.username) {
            (Some(user_id), Some(username)) => {
                let emails: Vec<UserEmail> = self
                    .user_emails
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();
                let orgs: Vec<CachedOrg> = self
                    .user_orgs
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or_default();
                Some(CachedUser {
                    user_id,
                    username,
                    profile_picture_url: None,
                    emails,
                    orgs,
                })
            }
            _ => None,
        };

        SessionData {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            access_expires_at: self.access_expires_at,
            user,
            csrf_token: self.csrf_token,
            created_at: self.created_at,
            last_seen_at: self.last_seen_at,
            needs_username: false,
        }
    }
}
