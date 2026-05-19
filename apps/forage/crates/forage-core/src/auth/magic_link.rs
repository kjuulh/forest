use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

/// Token type for the passwordless-login flow.
pub const TOKEN_TYPE_MAGIC_LINK: &str = "magic-link";
/// Token type for email-verification at signup (and post-signup add_email).
pub const TOKEN_TYPE_EMAIL_VERIFY: &str = "email-verify";

/// Errors from magic link token operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum MagicLinkError {
    #[error("store error: {0}")]
    Store(String),
}

/// Trait for token persistence shared by the magic-link login flow and
/// the email-verification flow. The `token_type` discriminator
/// segregates the two address-spaces; consumers cannot redeem a
/// magic-link token at the verify-email route or vice versa.
#[async_trait::async_trait]
pub trait MagicLinkStore: Send + Sync {
    /// Store a hashed token with its associated email and expiry, scoped
    /// to a specific `token_type`.
    async fn store_token(
        &self,
        token_type: &str,
        token_hash: &str,
        email: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<(), MagicLinkError>;

    /// Verify and consume a token atomically. Returns the email if a
    /// non-expired (token_type, token_hash) pair existed, None otherwise.
    async fn verify_and_consume(
        &self,
        token_type: &str,
        token_hash: &str,
    ) -> Result<Option<String>, MagicLinkError>;

    /// Count tokens of the given type created for this email since the
    /// given time (for per-type rate limiting).
    async fn count_recent(
        &self,
        token_type: &str,
        email: &str,
        since: DateTime<Utc>,
    ) -> Result<u64, MagicLinkError>;

    /// Remove expired tokens (across all types).
    async fn reap_expired(&self) -> Result<u64, MagicLinkError>;
}

/// Generate a magic link token.
/// Returns `(raw_token_base64url, sha256_hex_hash)`.
pub fn generate_magic_link_token() -> (String, String) {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    let raw = base64url_encode(&bytes);
    let hash = sha256_hex(raw.as_bytes());
    (raw, hash)
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

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Hash a raw token for storage/lookup.
pub fn hash_magic_link_token(raw: &str) -> String {
    sha256_hex(raw.as_bytes())
}

struct StoredToken {
    token_type: String,
    email: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

/// In-memory implementation for development and tests. Keys by
/// `(token_type, token_hash)` so the two address-spaces don't collide.
pub struct InMemoryMagicLinkStore {
    tokens: Mutex<HashMap<(String, String), StoredToken>>,
}

impl InMemoryMagicLinkStore {
    pub fn new() -> Self {
        Self {
            tokens: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl MagicLinkStore for InMemoryMagicLinkStore {
    async fn store_token(
        &self,
        token_type: &str,
        token_hash: &str,
        email: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<(), MagicLinkError> {
        let mut tokens = self.tokens.lock().unwrap();
        tokens.insert(
            (token_type.to_string(), token_hash.to_string()),
            StoredToken {
                token_type: token_type.to_string(),
                email: email.to_string(),
                expires_at,
                created_at: Utc::now(),
            },
        );
        Ok(())
    }

    async fn verify_and_consume(
        &self,
        token_type: &str,
        token_hash: &str,
    ) -> Result<Option<String>, MagicLinkError> {
        let mut tokens = self.tokens.lock().unwrap();
        if let Some(token) = tokens.remove(&(token_type.to_string(), token_hash.to_string())) {
            if token.expires_at > Utc::now() {
                return Ok(Some(token.email));
            }
        }
        Ok(None)
    }

    async fn count_recent(
        &self,
        token_type: &str,
        email: &str,
        since: DateTime<Utc>,
    ) -> Result<u64, MagicLinkError> {
        let tokens = self.tokens.lock().unwrap();
        let count = tokens
            .values()
            .filter(|t| t.token_type == token_type && t.email == email && t.created_at >= since)
            .count();
        Ok(count as u64)
    }

    async fn reap_expired(&self) -> Result<u64, MagicLinkError> {
        let mut tokens = self.tokens.lock().unwrap();
        let now = Utc::now();
        let before = tokens.len();
        tokens.retain(|_, t| t.expires_at > now);
        Ok((before - tokens.len()) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_token_produces_unique_pairs() {
        let (raw1, hash1) = generate_magic_link_token();
        let (raw2, hash2) = generate_magic_link_token();
        assert_ne!(raw1, raw2);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn hash_is_deterministic() {
        let (raw, hash) = generate_magic_link_token();
        assert_eq!(hash, hash_magic_link_token(&raw));
    }

    #[tokio::test]
    async fn in_memory_store_and_consume() {
        let store = InMemoryMagicLinkStore::new();
        let (raw, hash) = generate_magic_link_token();
        let expires = Utc::now() + chrono::Duration::minutes(15);

        store
            .store_token(TOKEN_TYPE_MAGIC_LINK, &hash, "test@example.com", expires)
            .await
            .unwrap();

        // First consume succeeds
        let email = store
            .verify_and_consume(TOKEN_TYPE_MAGIC_LINK, &hash)
            .await
            .unwrap();
        assert_eq!(email, Some("test@example.com".into()));

        // Second consume fails (single-use)
        let email = store
            .verify_and_consume(TOKEN_TYPE_MAGIC_LINK, &hash)
            .await
            .unwrap();
        assert_eq!(email, None);
        let _ = raw;
    }

    #[tokio::test]
    async fn expired_token_returns_none() {
        let store = InMemoryMagicLinkStore::new();
        let (_, hash) = generate_magic_link_token();
        let expired = Utc::now() - chrono::Duration::seconds(1);

        store
            .store_token(TOKEN_TYPE_MAGIC_LINK, &hash, "test@example.com", expired)
            .await
            .unwrap();

        let email = store
            .verify_and_consume(TOKEN_TYPE_MAGIC_LINK, &hash)
            .await
            .unwrap();
        assert_eq!(email, None);
    }

    #[tokio::test]
    async fn count_recent_tracks_per_email_and_type() {
        let store = InMemoryMagicLinkStore::new();
        let expires = Utc::now() + chrono::Duration::minutes(15);
        let since = Utc::now() - chrono::Duration::minutes(15);

        for _ in 0..3 {
            let (_, hash) = generate_magic_link_token();
            store
                .store_token(TOKEN_TYPE_MAGIC_LINK, &hash, "test@example.com", expires)
                .await
                .unwrap();
        }
        for _ in 0..2 {
            let (_, hash) = generate_magic_link_token();
            store
                .store_token(TOKEN_TYPE_EMAIL_VERIFY, &hash, "test@example.com", expires)
                .await
                .unwrap();
        }

        assert_eq!(
            store
                .count_recent(TOKEN_TYPE_MAGIC_LINK, "test@example.com", since)
                .await
                .unwrap(),
            3,
        );
        assert_eq!(
            store
                .count_recent(TOKEN_TYPE_EMAIL_VERIFY, "test@example.com", since)
                .await
                .unwrap(),
            2,
        );
        assert_eq!(
            store
                .count_recent(TOKEN_TYPE_MAGIC_LINK, "other@example.com", since)
                .await
                .unwrap(),
            0,
        );
    }

    #[tokio::test]
    async fn cross_type_redemption_is_rejected() {
        let store = InMemoryMagicLinkStore::new();
        let (_, hash) = generate_magic_link_token();
        let expires = Utc::now() + chrono::Duration::minutes(15);

        // Store as magic-link, attempt to redeem as email-verify.
        store
            .store_token(TOKEN_TYPE_MAGIC_LINK, &hash, "test@example.com", expires)
            .await
            .unwrap();

        let email = store
            .verify_and_consume(TOKEN_TYPE_EMAIL_VERIFY, &hash)
            .await
            .unwrap();
        assert_eq!(email, None, "cross-type redemption must fail");

        // Original type still works.
        let email = store
            .verify_and_consume(TOKEN_TYPE_MAGIC_LINK, &hash)
            .await
            .unwrap();
        assert_eq!(email, Some("test@example.com".into()));
    }
}
