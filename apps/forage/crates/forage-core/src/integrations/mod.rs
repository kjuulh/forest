pub mod email;
pub mod nats;
pub mod router;
pub mod webhook;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Integration types ────────────────────────────────────────────────

/// An org-level notification integration (Slack workspace, webhook URL, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Integration {
    pub id: String,
    pub organisation: String,
    pub integration_type: IntegrationType,
    pub name: String,
    pub config: IntegrationConfig,
    pub enabled: bool,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
    /// The raw API token, only populated when the integration is first created.
    /// After creation, this is None (only the hash is stored).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_token: Option<String>,
}

/// Supported integration types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationType {
    Slack,
    Webhook,
}

impl IntegrationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Slack => "slack",
            Self::Webhook => "webhook",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "slack" => Some(Self::Slack),
            "webhook" => Some(Self::Webhook),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Slack => "Slack",
            Self::Webhook => "Webhook",
        }
    }
}

/// Type-specific configuration for an integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IntegrationConfig {
    Slack {
        team_id: String,
        team_name: String,
        channel_id: String,
        channel_name: String,
        access_token: String,
        webhook_url: String,
    },
    Webhook {
        url: String,
        #[serde(default)]
        secret: Option<String>,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

// ── Notification rules ───────────────────────────────────────────────

/// Which event types an integration should receive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationRule {
    pub id: String,
    pub integration_id: String,
    pub notification_type: String,
    pub enabled: bool,
}

/// Known notification event types.
pub const NOTIFICATION_TYPES: &[&str] = &[
    "release_annotated",
    "release_started",
    "release_succeeded",
    "release_failed",
];

// ── Slack user links ─────────────────────────────────────────────────

/// Links a Forage user to their Slack identity in a workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackUserLink {
    pub id: String,
    pub user_id: String,        // Forage/Forest user ID
    pub team_id: String,        // Slack workspace ID
    pub team_name: String,      // Slack workspace name (display)
    pub slack_user_id: String,  // Slack user ID (U-xxx)
    pub slack_username: String, // Slack display name
    pub created_at: String,
}

/// Per-destination deployment status within a release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestinationStatus {
    pub environment: String,
    pub status: String, // "started", "succeeded", "failed"
    pub error: Option<String>,
}

/// Tracks a posted Slack message so we can update it in-place.
/// One ref per (integration, release_slug) — accumulates all destinations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackMessageRef {
    pub id: String,
    pub integration_id: String,
    pub release_id: String,      // release slug (shared across destinations)
    pub channel_id: String,      // Slack channel where posted
    pub message_ts: String,      // Slack message timestamp (for chat.update)
    pub last_event_type: String, // Last event that updated this message
    /// Accumulated per-destination statuses. Key = destination name.
    #[serde(default)]
    pub destinations: HashMap<String, DestinationStatus>,
    /// Cached release title for message rebuilds.
    #[serde(default)]
    pub release_title: String,
    pub created_at: String,
    pub updated_at: String,
}

// ── Delivery log ─────────────────────────────────────────────────────

/// Record of a notification delivery attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationDelivery {
    pub id: String,
    pub integration_id: String,
    pub notification_id: String,
    pub status: DeliveryStatus,
    pub error_message: Option<String>,
    pub attempted_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Delivered,
    Failed,
    Pending,
}

impl DeliveryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Failed => "failed",
            Self::Pending => "pending",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "delivered" => Some(Self::Delivered),
            "failed" => Some(Self::Failed),
            "pending" => Some(Self::Pending),
            _ => None,
        }
    }
}

// ── Create/Update inputs ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CreateIntegrationInput {
    pub organisation: String,
    pub integration_type: IntegrationType,
    pub name: String,
    pub config: IntegrationConfig,
    pub created_by: String,
}

// ── Error type ───────────────────────────────────────────────────────

#[derive(Debug, Clone, thiserror::Error)]
pub enum IntegrationError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("duplicate: {0}")]
    Duplicate(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("store error: {0}")]
    Store(String),

    #[error("encryption error: {0}")]
    Encryption(String),
}

// ── Repository trait ─────────────────────────────────────────────────

/// Persistence trait for integration management. Implemented by forage-db.
#[async_trait::async_trait]
pub trait IntegrationStore: Send + Sync {
    /// List all integrations for an organisation.
    async fn list_integrations(
        &self,
        organisation: &str,
    ) -> Result<Vec<Integration>, IntegrationError>;

    /// Get a single integration by ID (must belong to the given org).
    async fn get_integration(
        &self,
        organisation: &str,
        id: &str,
    ) -> Result<Integration, IntegrationError>;

    /// Create a new integration with default notification rules (all enabled).
    async fn create_integration(
        &self,
        input: &CreateIntegrationInput,
    ) -> Result<Integration, IntegrationError>;

    /// Enable or disable an integration.
    async fn set_integration_enabled(
        &self,
        organisation: &str,
        id: &str,
        enabled: bool,
    ) -> Result<(), IntegrationError>;

    /// Delete an integration and its rules/deliveries (cascading).
    async fn delete_integration(
        &self,
        organisation: &str,
        id: &str,
    ) -> Result<(), IntegrationError>;

    /// List notification rules for an integration.
    async fn list_rules(
        &self,
        integration_id: &str,
    ) -> Result<Vec<NotificationRule>, IntegrationError>;

    /// Set whether a specific notification type is enabled for an integration.
    async fn set_rule_enabled(
        &self,
        integration_id: &str,
        notification_type: &str,
        enabled: bool,
    ) -> Result<(), IntegrationError>;

    /// Record a delivery attempt.
    async fn record_delivery(
        &self,
        integration_id: &str,
        notification_id: &str,
        status: DeliveryStatus,
        error_message: Option<&str>,
    ) -> Result<(), IntegrationError>;

    /// List enabled integrations for an org that have a matching rule for the given event type.
    async fn list_matching_integrations(
        &self,
        organisation: &str,
        notification_type: &str,
    ) -> Result<Vec<Integration>, IntegrationError>;

    /// List recent delivery attempts for an integration, newest first.
    async fn list_deliveries(
        &self,
        integration_id: &str,
        limit: usize,
    ) -> Result<Vec<NotificationDelivery>, IntegrationError>;

    /// Update the configuration (and optionally the name) of an existing integration.
    async fn update_integration_config(
        &self,
        organisation: &str,
        id: &str,
        name: &str,
        config: &IntegrationConfig,
    ) -> Result<(), IntegrationError>;

    /// Look up an integration by its API token hash. Used for API authentication.
    async fn get_integration_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Integration, IntegrationError>;

    // ── Slack user links ──────────────────────────────────────────────

    /// Get the Slack user link for a Forage user in a given workspace, if any.
    async fn get_slack_user_link(
        &self,
        user_id: &str,
        team_id: &str,
    ) -> Result<Option<SlackUserLink>, IntegrationError>;

    /// Create or update the Slack user link for a given (user_id, team_id) pair.
    async fn upsert_slack_user_link(&self, link: &SlackUserLink) -> Result<(), IntegrationError>;

    /// Remove the Slack user link for a given (user_id, team_id) pair.
    async fn delete_slack_user_link(
        &self,
        user_id: &str,
        team_id: &str,
    ) -> Result<(), IntegrationError>;

    /// List all Slack user links for a Forage user (one per connected workspace).
    async fn list_slack_user_links(
        &self,
        user_id: &str,
    ) -> Result<Vec<SlackUserLink>, IntegrationError>;

    // ── Slack message refs ────────────────────────────────────────────

    /// Get the Slack message ref for a release in a specific integration, if any.
    async fn get_slack_message_ref(
        &self,
        integration_id: &str,
        release_id: &str,
    ) -> Result<Option<SlackMessageRef>, IntegrationError>;

    /// Create or update the Slack message ref for a given (integration_id, release_id) pair.
    async fn upsert_slack_message_ref(
        &self,
        msg_ref: &SlackMessageRef,
    ) -> Result<(), IntegrationError>;
}

// ── Token generation ────────────────────────────────────────────────

/// Generate a crypto-random API token for an integration.
/// Format: `fgi_` prefix + 32 bytes hex-encoded.
pub fn generate_api_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let encoded = hex_encode(&bytes);
    format!("fgi_{encoded}")
}

/// SHA-256 hash of a token for storage. Only the hash is persisted.
pub fn hash_api_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(token.as_bytes());
    hex_encode(&hash)
}

fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

// ── Validation ───────────────────────────────────────────────────────

/// Validate a webhook URL. Must be HTTPS (or localhost for development).
pub fn validate_webhook_url(url: &str) -> Result<(), IntegrationError> {
    if url.starts_with("https://") {
        return Ok(());
    }
    if url.starts_with("http://localhost") || url.starts_with("http://127.0.0.1") {
        return Ok(());
    }
    Err(IntegrationError::InvalidInput(
        "Webhook URL must use HTTPS".to_string(),
    ))
}

/// Validate an integration name (reuse slug rules: lowercase alphanumeric + hyphens, max 64).
pub fn validate_integration_name(name: &str) -> Result<(), IntegrationError> {
    if name.is_empty() {
        return Err(IntegrationError::InvalidInput(
            "Integration name cannot be empty".to_string(),
        ));
    }
    if name.len() > 64 {
        return Err(IntegrationError::InvalidInput(
            "Integration name too long (max 64 characters)".to_string(),
        ));
    }
    // Allow more characters than slugs: spaces, #, etc. for human-readable names
    if name.chars().any(|c| c.is_control()) {
        return Err(IntegrationError::InvalidInput(
            "Integration name contains invalid characters".to_string(),
        ));
    }
    Ok(())
}

// ── In-memory store (for tests) ──────────────────────────────────────

/// In-memory integration store for testing. Not for production use.
pub struct InMemoryIntegrationStore {
    integrations: std::sync::Mutex<Vec<Integration>>,
    rules: std::sync::Mutex<Vec<NotificationRule>>,
    deliveries: std::sync::Mutex<Vec<NotificationDelivery>>,
    /// Stores token_hash -> integration_id for lookup.
    token_hashes: std::sync::Mutex<HashMap<String, String>>,
    slack_user_links: std::sync::Mutex<Vec<SlackUserLink>>,
    slack_message_refs: std::sync::Mutex<Vec<SlackMessageRef>>,
}

impl InMemoryIntegrationStore {
    pub fn new() -> Self {
        Self {
            integrations: std::sync::Mutex::new(Vec::new()),
            rules: std::sync::Mutex::new(Vec::new()),
            deliveries: std::sync::Mutex::new(Vec::new()),
            token_hashes: std::sync::Mutex::new(HashMap::new()),
            slack_user_links: std::sync::Mutex::new(Vec::new()),
            slack_message_refs: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Default for InMemoryIntegrationStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Prefix for integration API tokens.
pub const TOKEN_PREFIX: &str = "fgi_";

#[async_trait::async_trait]
impl IntegrationStore for InMemoryIntegrationStore {
    async fn list_integrations(
        &self,
        organisation: &str,
    ) -> Result<Vec<Integration>, IntegrationError> {
        let store = self.integrations.lock().unwrap();
        Ok(store
            .iter()
            .filter(|i| i.organisation == organisation)
            .cloned()
            .collect())
    }

    async fn get_integration(
        &self,
        organisation: &str,
        id: &str,
    ) -> Result<Integration, IntegrationError> {
        let store = self.integrations.lock().unwrap();
        store
            .iter()
            .find(|i| i.id == id && i.organisation == organisation)
            .cloned()
            .ok_or_else(|| IntegrationError::NotFound(id.to_string()))
    }

    async fn create_integration(
        &self,
        input: &CreateIntegrationInput,
    ) -> Result<Integration, IntegrationError> {
        let mut store = self.integrations.lock().unwrap();
        if store
            .iter()
            .any(|i| i.organisation == input.organisation && i.name == input.name)
        {
            return Err(IntegrationError::Duplicate(format!(
                "Integration '{}' already exists",
                input.name
            )));
        }

        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let raw_token = generate_api_token();
        let token_hash = hash_api_token(&raw_token);

        let integration = Integration {
            id: id.clone(),
            organisation: input.organisation.clone(),
            integration_type: input.integration_type,
            name: input.name.clone(),
            config: input.config.clone(),
            enabled: true,
            created_by: input.created_by.clone(),
            created_at: now.clone(),
            updated_at: now,
            api_token: Some(raw_token),
        };
        // Store without the raw token
        let stored = Integration { api_token: None, ..integration.clone() };
        store.push(stored);

        // Store token hash
        self.token_hashes.lock().unwrap().insert(token_hash, id.clone());

        // Create default rules
        let mut rules = self.rules.lock().unwrap();
        for nt in NOTIFICATION_TYPES {
            rules.push(NotificationRule {
                id: uuid::Uuid::new_v4().to_string(),
                integration_id: id.clone(),
                notification_type: nt.to_string(),
                enabled: true,
            });
        }

        Ok(integration)
    }

    async fn set_integration_enabled(
        &self,
        organisation: &str,
        id: &str,
        enabled: bool,
    ) -> Result<(), IntegrationError> {
        let mut store = self.integrations.lock().unwrap();
        let integ = store
            .iter_mut()
            .find(|i| i.id == id && i.organisation == organisation)
            .ok_or_else(|| IntegrationError::NotFound(id.to_string()))?;
        integ.enabled = enabled;
        Ok(())
    }

    async fn delete_integration(
        &self,
        organisation: &str,
        id: &str,
    ) -> Result<(), IntegrationError> {
        let mut store = self.integrations.lock().unwrap();
        let len = store.len();
        store.retain(|i| !(i.id == id && i.organisation == organisation));
        if store.len() == len {
            return Err(IntegrationError::NotFound(id.to_string()));
        }
        // Cascade delete rules
        let mut rules = self.rules.lock().unwrap();
        rules.retain(|r| r.integration_id != id);
        Ok(())
    }

    async fn update_integration_config(
        &self,
        organisation: &str,
        id: &str,
        name: &str,
        config: &IntegrationConfig,
    ) -> Result<(), IntegrationError> {
        let mut store = self.integrations.lock().unwrap();
        let integ = store
            .iter_mut()
            .find(|i| i.id == id && i.organisation == organisation)
            .ok_or_else(|| IntegrationError::NotFound(id.to_string()))?;
        integ.name = name.to_string();
        integ.config = config.clone();
        Ok(())
    }

    async fn list_rules(
        &self,
        integration_id: &str,
    ) -> Result<Vec<NotificationRule>, IntegrationError> {
        let rules = self.rules.lock().unwrap();
        Ok(rules
            .iter()
            .filter(|r| r.integration_id == integration_id)
            .cloned()
            .collect())
    }

    async fn set_rule_enabled(
        &self,
        integration_id: &str,
        notification_type: &str,
        enabled: bool,
    ) -> Result<(), IntegrationError> {
        let mut rules = self.rules.lock().unwrap();
        if let Some(rule) = rules
            .iter_mut()
            .find(|r| r.integration_id == integration_id && r.notification_type == notification_type)
        {
            rule.enabled = enabled;
        } else {
            rules.push(NotificationRule {
                id: uuid::Uuid::new_v4().to_string(),
                integration_id: integration_id.to_string(),
                notification_type: notification_type.to_string(),
                enabled,
            });
        }
        Ok(())
    }

    async fn record_delivery(
        &self,
        integration_id: &str,
        notification_id: &str,
        status: DeliveryStatus,
        error_message: Option<&str>,
    ) -> Result<(), IntegrationError> {
        let mut deliveries = self.deliveries.lock().unwrap();
        deliveries.push(NotificationDelivery {
            id: uuid::Uuid::new_v4().to_string(),
            integration_id: integration_id.to_string(),
            notification_id: notification_id.to_string(),
            status,
            error_message: error_message.map(|s| s.to_string()),
            attempted_at: chrono::Utc::now().to_rfc3339(),
        });
        Ok(())
    }

    async fn list_deliveries(
        &self,
        integration_id: &str,
        limit: usize,
    ) -> Result<Vec<NotificationDelivery>, IntegrationError> {
        let deliveries = self.deliveries.lock().unwrap();
        let mut matching: Vec<_> = deliveries
            .iter()
            .filter(|d| d.integration_id == integration_id)
            .cloned()
            .collect();
        // Sort newest first (by attempted_at descending)
        matching.sort_by(|a, b| b.attempted_at.cmp(&a.attempted_at));
        matching.truncate(limit);
        Ok(matching)
    }

    async fn list_matching_integrations(
        &self,
        organisation: &str,
        notification_type: &str,
    ) -> Result<Vec<Integration>, IntegrationError> {
        let store = self.integrations.lock().unwrap();
        let rules = self.rules.lock().unwrap();
        Ok(store
            .iter()
            .filter(|i| {
                i.organisation == organisation
                    && i.enabled
                    && rules.iter().any(|r| {
                        r.integration_id == i.id
                            && r.notification_type == notification_type
                            && r.enabled
                    })
            })
            .cloned()
            .collect())
    }

    async fn get_integration_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Integration, IntegrationError> {
        let hashes = self.token_hashes.lock().unwrap();
        let id = hashes
            .get(token_hash)
            .ok_or_else(|| IntegrationError::NotFound("invalid token".to_string()))?
            .clone();
        drop(hashes);

        let store = self.integrations.lock().unwrap();
        store
            .iter()
            .find(|i| i.id == id)
            .cloned()
            .ok_or(IntegrationError::NotFound(id))
    }

    async fn get_slack_user_link(
        &self,
        user_id: &str,
        team_id: &str,
    ) -> Result<Option<SlackUserLink>, IntegrationError> {
        let links = self.slack_user_links.lock().unwrap();
        Ok(links
            .iter()
            .find(|l| l.user_id == user_id && l.team_id == team_id)
            .cloned())
    }

    async fn upsert_slack_user_link(&self, link: &SlackUserLink) -> Result<(), IntegrationError> {
        let mut links = self.slack_user_links.lock().unwrap();
        if let Some(existing) = links
            .iter_mut()
            .find(|l| l.user_id == link.user_id && l.team_id == link.team_id)
        {
            *existing = link.clone();
        } else {
            links.push(link.clone());
        }
        Ok(())
    }

    async fn delete_slack_user_link(
        &self,
        user_id: &str,
        team_id: &str,
    ) -> Result<(), IntegrationError> {
        let mut links = self.slack_user_links.lock().unwrap();
        links.retain(|l| !(l.user_id == user_id && l.team_id == team_id));
        Ok(())
    }

    async fn list_slack_user_links(
        &self,
        user_id: &str,
    ) -> Result<Vec<SlackUserLink>, IntegrationError> {
        let links = self.slack_user_links.lock().unwrap();
        Ok(links
            .iter()
            .filter(|l| l.user_id == user_id)
            .cloned()
            .collect())
    }

    async fn get_slack_message_ref(
        &self,
        integration_id: &str,
        release_id: &str,
    ) -> Result<Option<SlackMessageRef>, IntegrationError> {
        let refs = self.slack_message_refs.lock().unwrap();
        Ok(refs
            .iter()
            .find(|r| r.integration_id == integration_id && r.release_id == release_id)
            .cloned())
    }

    async fn upsert_slack_message_ref(
        &self,
        msg_ref: &SlackMessageRef,
    ) -> Result<(), IntegrationError> {
        let mut refs = self.slack_message_refs.lock().unwrap();
        if let Some(existing) = refs.iter_mut().find(|r| {
            r.integration_id == msg_ref.integration_id && r.release_id == msg_ref.release_id
        }) {
            *existing = msg_ref.clone();
        } else {
            refs.push(msg_ref.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integration_type_roundtrip() {
        for t in &[IntegrationType::Slack, IntegrationType::Webhook] {
            let s = t.as_str();
            assert_eq!(IntegrationType::parse(s), Some(*t));
        }
    }

    #[test]
    fn integration_type_unknown_returns_none() {
        assert_eq!(IntegrationType::parse("discord"), None);
        assert_eq!(IntegrationType::parse(""), None);
    }

    #[test]
    fn delivery_status_roundtrip() {
        for s in &[
            DeliveryStatus::Delivered,
            DeliveryStatus::Failed,
            DeliveryStatus::Pending,
        ] {
            let str = s.as_str();
            assert_eq!(DeliveryStatus::parse(str), Some(*s));
        }
    }

    #[test]
    fn validate_webhook_url_https() {
        assert!(validate_webhook_url("https://example.com/hook").is_ok());
    }

    #[test]
    fn validate_webhook_url_localhost() {
        assert!(validate_webhook_url("http://localhost:8080/hook").is_ok());
        assert!(validate_webhook_url("http://127.0.0.1:8080/hook").is_ok());
    }

    #[test]
    fn validate_webhook_url_http_rejected() {
        assert!(validate_webhook_url("http://example.com/hook").is_err());
    }

    #[test]
    fn validate_integration_name_valid() {
        assert!(validate_integration_name("my-slack").is_ok());
        assert!(validate_integration_name("#deploys").is_ok());
        assert!(validate_integration_name("Production alerts").is_ok());
    }

    #[test]
    fn validate_integration_name_empty() {
        assert!(validate_integration_name("").is_err());
    }

    #[test]
    fn validate_integration_name_too_long() {
        assert!(validate_integration_name(&"a".repeat(65)).is_err());
    }

    #[test]
    fn validate_integration_name_control_chars() {
        assert!(validate_integration_name("bad\x00name").is_err());
    }

    #[test]
    fn integration_config_slack_serde_roundtrip() {
        let config = IntegrationConfig::Slack {
            team_id: "T123".into(),
            team_name: "My Team".into(),
            channel_id: "C456".into(),
            channel_name: "#deploys".into(),
            access_token: "xoxb-token".into(),
            webhook_url: "https://hooks.slack.com/...".into(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: IntegrationConfig = serde_json::from_str(&json).unwrap();
        match parsed {
            IntegrationConfig::Slack { team_id, .. } => assert_eq!(team_id, "T123"),
            _ => panic!("expected Slack config"),
        }
    }

    #[test]
    fn integration_config_webhook_serde_roundtrip() {
        let config = IntegrationConfig::Webhook {
            url: "https://example.com/hook".into(),
            secret: Some("s3cret".into()),
            headers: HashMap::from([("X-Custom".into(), "value".into())]),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: IntegrationConfig = serde_json::from_str(&json).unwrap();
        match parsed {
            IntegrationConfig::Webhook { url, secret, headers } => {
                assert_eq!(url, "https://example.com/hook");
                assert_eq!(secret.as_deref(), Some("s3cret"));
                assert_eq!(headers.get("X-Custom").map(|s| s.as_str()), Some("value"));
            }
            _ => panic!("expected Webhook config"),
        }
    }

    #[test]
    fn notification_types_are_known() {
        assert_eq!(NOTIFICATION_TYPES.len(), 4);
        assert!(NOTIFICATION_TYPES.contains(&"release_failed"));
    }

    #[test]
    fn generate_api_token_has_prefix_and_length() {
        let token = generate_api_token();
        assert!(token.starts_with("fgi_"));
        // fgi_ (4) + 64 hex chars (32 bytes) = 68 total
        assert_eq!(token.len(), 68);
    }

    #[test]
    fn generate_api_token_is_unique() {
        let t1 = generate_api_token();
        let t2 = generate_api_token();
        assert_ne!(t1, t2);
    }

    #[test]
    fn hash_api_token_is_deterministic() {
        let token = "fgi_abcdef1234567890";
        let h1 = hash_api_token(token);
        let h2 = hash_api_token(token);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 = 32 bytes = 64 hex chars
    }

    #[test]
    fn hash_api_token_different_for_different_tokens() {
        let h1 = hash_api_token("fgi_token_one");
        let h2 = hash_api_token("fgi_token_two");
        assert_ne!(h1, h2);
    }

    #[tokio::test]
    async fn in_memory_store_creates_with_api_token() {
        let store = InMemoryIntegrationStore::new();
        let created = store
            .create_integration(&CreateIntegrationInput {
                organisation: "myorg".into(),
                integration_type: IntegrationType::Webhook,
                name: "test-hook".into(),
                config: IntegrationConfig::Webhook {
                    url: "https://example.com/hook".into(),
                    secret: None,
                    headers: HashMap::new(),
                },
                created_by: "user-1".into(),
            })
            .await
            .unwrap();

        // Token is returned on creation
        assert!(created.api_token.is_some());
        let token = created.api_token.unwrap();
        assert!(token.starts_with("fgi_"));

        // Token lookup works
        let token_hash = hash_api_token(&token);
        let found = store.get_integration_by_token_hash(&token_hash).await.unwrap();
        assert_eq!(found.id, created.id);
        assert!(found.api_token.is_none()); // not stored in plaintext

        // Stored integration doesn't have the raw token
        let listed = store.list_integrations("myorg").await.unwrap();
        assert!(listed[0].api_token.is_none());
    }
}
