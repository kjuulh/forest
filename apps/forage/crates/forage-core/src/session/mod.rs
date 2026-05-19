mod file_store;
mod store;

pub use file_store::FileSessionStore;
pub use store::InMemorySessionStore;

use crate::auth::UserEmail;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Opaque session identifier. 32 bytes of cryptographic randomness, base64url-encoded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    pub fn generate() -> Self {
        use rand::Rng;
        let mut bytes = [0u8; 32];
        rand::rng().fill(&mut bytes);
        Self(base64url_encode(&bytes))
    }

    /// Construct from a raw cookie value. No validation - it's just a lookup key.
    pub fn from_raw(s: String) -> Self {
        Self(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn base64url_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let n = match chunk.len() {
            3 => (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8 | chunk[2] as u32,
            2 => (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8,
            1 => (chunk[0] as u32) << 16,
            _ => unreachable!(),
        };
        let _ = out.write_char(CHARS[((n >> 18) & 0x3F) as usize] as char);
        let _ = out.write_char(CHARS[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            let _ = out.write_char(CHARS[((n >> 6) & 0x3F) as usize] as char);
        }
        if chunk.len() > 2 {
            let _ = out.write_char(CHARS[(n & 0x3F) as usize] as char);
        }
    }
    out
}

/// Cached user info stored in the session to avoid repeated gRPC calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedUser {
    pub user_id: String,
    pub username: String,
    #[serde(default)]
    pub profile_picture_url: Option<String>,
    pub emails: Vec<UserEmail>,
    #[serde(default)]
    pub orgs: Vec<CachedOrg>,
}

/// Cached organisation membership.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedOrg {
    #[serde(default)]
    pub organisation_id: String,
    pub name: String,
    pub role: String,
}

/// Generate a CSRF token (16 bytes of randomness, base64url-encoded).
pub fn generate_csrf_token() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    base64url_encode(&bytes)
}

/// Server-side session data. Never exposed to the browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at: DateTime<Utc>,
    pub user: Option<CachedUser>,
    pub csrf_token: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    /// True when a user signed up via OAuth and still needs to pick a username.
    #[serde(default)]
    pub needs_username: bool,
}

impl SessionData {
    /// Whether the access token is expired or will expire within the given margin.
    pub fn is_access_expired(&self, margin: chrono::Duration) -> bool {
        Utc::now() + margin >= self.access_expires_at
    }

    /// Whether the access token needs refreshing (expired or within 60s of expiry).
    pub fn needs_refresh(&self) -> bool {
        self.is_access_expired(chrono::Duration::seconds(60))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session store error: {0}")]
    Store(String),
}

/// Trait for session persistence. Swappable between in-memory, Redis, Postgres.
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    async fn create(&self, data: SessionData) -> Result<SessionId, SessionError>;
    async fn get(&self, id: &SessionId) -> Result<Option<SessionData>, SessionError>;
    async fn update(&self, id: &SessionId, data: SessionData) -> Result<(), SessionError>;
    async fn delete(&self, id: &SessionId) -> Result<(), SessionError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn session_id_generates_unique_ids() {
        let ids: HashSet<String> = (0..1000).map(|_| SessionId::generate().0).collect();
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn session_id_is_base64url_safe() {
        for _ in 0..100 {
            let id = SessionId::generate();
            let s = id.as_str();
            assert!(
                s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "invalid chars in session id: {s}"
            );
        }
    }

    #[test]
    fn session_id_has_sufficient_length() {
        // 32 bytes -> ~43 base64url chars
        let id = SessionId::generate();
        assert!(id.as_str().len() >= 42, "session id too short: {}", id.as_str().len());
    }

    #[test]
    fn session_data_not_expired() {
        let data = SessionData {
            access_token: "tok".into(),
            refresh_token: "ref".into(),
            csrf_token: "test-csrf".into(),
            needs_username: false,
            access_expires_at: Utc::now() + chrono::Duration::hours(1),
            user: None,
            created_at: Utc::now(),
            last_seen_at: Utc::now(),
        };
        assert!(!data.is_access_expired(chrono::Duration::zero()));
        assert!(!data.needs_refresh());
    }

    #[test]
    fn session_data_expired() {
        let data = SessionData {
            access_token: "tok".into(),
            refresh_token: "ref".into(),
            csrf_token: "test-csrf".into(),
            needs_username: false,
            access_expires_at: Utc::now() - chrono::Duration::seconds(1),
            user: None,
            created_at: Utc::now(),
            last_seen_at: Utc::now(),
        };
        assert!(data.is_access_expired(chrono::Duration::zero()));
        assert!(data.needs_refresh());
    }

    #[test]
    fn session_data_needs_refresh_within_margin() {
        let data = SessionData {
            access_token: "tok".into(),
            refresh_token: "ref".into(),
            csrf_token: "test-csrf".into(),
            needs_username: false,
            access_expires_at: Utc::now() + chrono::Duration::seconds(30),
            user: None,
            created_at: Utc::now(),
            last_seen_at: Utc::now(),
        };
        // Not expired yet, but within 60s margin
        assert!(!data.is_access_expired(chrono::Duration::zero()));
        assert!(data.needs_refresh());
    }

    #[tokio::test]
    async fn in_memory_store_create_and_get() {
        let store = InMemorySessionStore::new();
        let data = make_session_data();
        let id = store.create(data.clone()).await.unwrap();
        let retrieved = store.get(&id).await.unwrap().expect("session should exist");
        assert_eq!(retrieved.access_token, data.access_token);
        assert_eq!(retrieved.refresh_token, data.refresh_token);
    }

    #[tokio::test]
    async fn in_memory_store_get_nonexistent_returns_none() {
        let store = InMemorySessionStore::new();
        let id = SessionId::generate();
        assert!(store.get(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn in_memory_store_update() {
        let store = InMemorySessionStore::new();
        let data = make_session_data();
        let id = store.create(data).await.unwrap();

        let mut updated = make_session_data();
        updated.access_token = "new-access".into();
        store.update(&id, updated).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.access_token, "new-access");
    }

    #[tokio::test]
    async fn in_memory_store_delete() {
        let store = InMemorySessionStore::new();
        let data = make_session_data();
        let id = store.create(data).await.unwrap();
        store.delete(&id).await.unwrap();
        assert!(store.get(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn in_memory_store_delete_nonexistent_is_ok() {
        let store = InMemorySessionStore::new();
        let id = SessionId::generate();
        // Should not error
        store.delete(&id).await.unwrap();
    }

    fn make_session_data() -> SessionData {
        SessionData {
            access_token: "test-access".into(),
            refresh_token: "test-refresh".into(),
            csrf_token: "test-csrf".into(),
            needs_username: false,
            access_expires_at: Utc::now() + chrono::Duration::hours(1),
            user: None,
            created_at: Utc::now(),
            last_seen_at: Utc::now(),
        }
    }
}
