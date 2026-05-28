//! Per-flow state for OAuth and MFA redirects.
//!
//! Each OAuth/MFA flow gets a fresh, opaque state token. The token is
//! the lookup key for any application-level intent that must survive the
//! browser detour — primarily a `return_to` path so a device-login user
//! lands on `/device?user_code=…` after authenticating, not `/dashboard`.
//!
//! Why a store (and not a cookie keyed by the state) — cookies are
//! global-per-browser, so two tabs racing through OAuth overwrite each
//! other's intent. The state token is already per-flow; binding intent
//! to it is tab-safe.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};

/// Provider discriminators used by callers.
pub const PROVIDER_GOOGLE: &str = "google";
pub const PROVIDER_GITHUB: &str = "github";
pub const PROVIDER_MFA: &str = "mfa";

/// Errors from oauth state store operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum OAuthStateError {
    #[error("store error: {0}")]
    Store(String),
}

/// Data persisted for a single in-flight OAuth/MFA flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthFlowState {
    pub return_to: Option<String>,
}

/// Trait for per-flow OAuth state persistence.
#[async_trait::async_trait]
pub trait OAuthStateStore: Send + Sync {
    /// Persist a new flow keyed by `(provider, state)`.
    async fn create(
        &self,
        provider: &str,
        state: &str,
        return_to: Option<&str>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), OAuthStateError>;

    /// Atomically read-and-delete the flow. Returns `None` if missing or
    /// expired or if the provider does not match (cross-provider
    /// redemption is rejected as defence in depth).
    async fn consume(
        &self,
        provider: &str,
        state: &str,
    ) -> Result<Option<OAuthFlowState>, OAuthStateError>;

    /// Remove expired rows (housekeeping; intended for the session reaper).
    async fn reap_expired(&self) -> Result<u64, OAuthStateError>;
}

struct StoredFlow {
    provider: String,
    return_to: Option<String>,
    expires_at: DateTime<Utc>,
}

/// In-memory store for development and tests. Keys by `(provider, state)`
/// so the same nonce can't be reused across providers (matches the PG
/// table's composite PK semantics).
pub struct InMemoryOAuthStateStore {
    flows: Mutex<HashMap<(String, String), StoredFlow>>,
}

impl InMemoryOAuthStateStore {
    pub fn new() -> Self {
        Self {
            flows: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryOAuthStateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl OAuthStateStore for InMemoryOAuthStateStore {
    async fn create(
        &self,
        provider: &str,
        state: &str,
        return_to: Option<&str>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), OAuthStateError> {
        let mut flows = self.flows.lock().unwrap();
        flows.insert(
            (provider.to_string(), state.to_string()),
            StoredFlow {
                provider: provider.to_string(),
                return_to: return_to.map(|s| s.to_string()),
                expires_at,
            },
        );
        Ok(())
    }

    async fn consume(
        &self,
        provider: &str,
        state: &str,
    ) -> Result<Option<OAuthFlowState>, OAuthStateError> {
        let mut flows = self.flows.lock().unwrap();
        if let Some(flow) = flows.remove(&(provider.to_string(), state.to_string())) {
            if flow.provider == provider && flow.expires_at > Utc::now() {
                return Ok(Some(OAuthFlowState {
                    return_to: flow.return_to,
                }));
            }
        }
        Ok(None)
    }

    async fn reap_expired(&self) -> Result<u64, OAuthStateError> {
        let mut flows = self.flows.lock().unwrap();
        let now = Utc::now();
        let before = flows.len();
        flows.retain(|_, f| f.expires_at > now);
        Ok((before - flows.len()) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_then_consume_returns_return_to() {
        let store = InMemoryOAuthStateStore::new();
        let expires = Utc::now() + chrono::Duration::minutes(10);
        store
            .create(
                PROVIDER_GOOGLE,
                "state-abc",
                Some("/device?user_code=ABCD-EFGH"),
                expires,
            )
            .await
            .unwrap();

        let consumed = store.consume(PROVIDER_GOOGLE, "state-abc").await.unwrap();
        assert_eq!(
            consumed,
            Some(OAuthFlowState {
                return_to: Some("/device?user_code=ABCD-EFGH".into()),
            })
        );
    }

    #[tokio::test]
    async fn consume_is_single_use() {
        let store = InMemoryOAuthStateStore::new();
        let expires = Utc::now() + chrono::Duration::minutes(10);
        store
            .create(PROVIDER_GOOGLE, "state-once", None, expires)
            .await
            .unwrap();

        assert!(
            store
                .consume(PROVIDER_GOOGLE, "state-once")
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            store.consume(PROVIDER_GOOGLE, "state-once").await.unwrap(),
            None,
        );
    }

    #[tokio::test]
    async fn cross_provider_redemption_is_rejected() {
        let store = InMemoryOAuthStateStore::new();
        let expires = Utc::now() + chrono::Duration::minutes(10);
        store
            .create(PROVIDER_GOOGLE, "shared-state", Some("/x"), expires)
            .await
            .unwrap();

        assert_eq!(
            store.consume(PROVIDER_GITHUB, "shared-state").await.unwrap(),
            None,
        );
        assert!(
            store
                .consume(PROVIDER_GOOGLE, "shared-state")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn expired_flow_yields_none() {
        let store = InMemoryOAuthStateStore::new();
        let expired = Utc::now() - chrono::Duration::seconds(1);
        store
            .create(PROVIDER_GOOGLE, "stale", Some("/x"), expired)
            .await
            .unwrap();

        assert_eq!(
            store.consume(PROVIDER_GOOGLE, "stale").await.unwrap(),
            None,
        );
    }
}
