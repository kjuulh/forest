pub mod baseline;
pub mod direct;
pub mod email;
pub mod nats;
pub mod router;
pub mod signing;
pub mod slack_release;
pub mod webhook;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationType {
    Slack,
    Webhook,
    #[serde(rename = "github")]
    GitHub,
}

impl IntegrationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Slack => "slack",
            Self::Webhook => "webhook",
            Self::GitHub => "github",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "slack" => Some(Self::Slack),
            "webhook" => Some(Self::Webhook),
            "github" => Some(Self::GitHub),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Slack => "Slack",
            Self::Webhook => "Webhook",
            Self::GitHub => "GitHub",
        }
    }
}

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
    /// A GitHub App installation on one organisation account. No token is
    /// stored: installation tokens are minted from the app key when needed.
    #[serde(rename = "github")]
    GitHub {
        installation_id: i64,
        account_id: i64,
        account_login: String,
        installed_by_login: String,
        #[serde(default)]
        suspended: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationRule {
    pub id: String,
    pub integration_id: String,
    pub notification_type: String,
    pub enabled: bool,
}

pub const NOTIFICATION_TYPES: &[&str] = &[
    "release_annotated",
    "release_started",
    "release_succeeded",
    "release_failed",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackUserLink {
    pub id: String,
    pub user_id: String,
    pub team_id: String,
    pub team_name: String,
    pub slack_user_id: String,
    pub slack_username: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DestinationStatus {
    pub environment: String,
    pub status: String,
    pub error: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackMessageRef {
    pub id: String,
    pub integration_id: String,
    pub release_id: String,
    pub channel_id: String,
    pub message_ts: String,
    pub last_event_type: String,
    #[serde(default)]
    pub destinations: HashMap<String, DestinationStatus>,
    #[serde(default)]
    pub release_title: String,
    /// Thread replies already posted under this message. Written only by
    /// `claim_slack_thread_key`, never by `upsert_slack_message_ref`.
    #[serde(default)]
    pub thread_keys: Vec<String>,
    #[serde(default)]
    pub rendered_hash: String,
    #[serde(default)]
    pub destination_count: i32,
    pub created_at: String,
    pub updated_at: String,
}

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
    Skipped,
}

impl DeliveryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Failed => "failed",
            Self::Pending => "pending",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "delivered" => Some(Self::Delivered),
            "failed" => Some(Self::Failed),
            "pending" => Some(Self::Pending),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateIntegrationInput {
    pub organisation: String,
    pub integration_type: IntegrationType,
    pub name: String,
    pub config: IntegrationConfig,
    pub created_by: String,
}

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

#[async_trait::async_trait]
pub trait IntegrationStore: Send + Sync {
    async fn list_integrations(
        &self,
        organisation: &str,
    ) -> Result<Vec<Integration>, IntegrationError>;

    /// Must only return an integration that belongs to `organisation`.
    async fn get_integration(
        &self,
        organisation: &str,
        id: &str,
    ) -> Result<Integration, IntegrationError>;

    /// Enables every notification rule by default.
    async fn create_integration(
        &self,
        input: &CreateIntegrationInput,
    ) -> Result<Integration, IntegrationError>;

    async fn set_integration_enabled(
        &self,
        organisation: &str,
        id: &str,
        enabled: bool,
    ) -> Result<(), IntegrationError>;

    async fn delete_integration(
        &self,
        organisation: &str,
        id: &str,
    ) -> Result<(), IntegrationError>;

    async fn list_rules(
        &self,
        integration_id: &str,
    ) -> Result<Vec<NotificationRule>, IntegrationError>;

    async fn set_rule_enabled(
        &self,
        integration_id: &str,
        notification_type: &str,
        enabled: bool,
    ) -> Result<(), IntegrationError>;

    async fn record_delivery(
        &self,
        integration_id: &str,
        notification_id: &str,
        status: DeliveryStatus,
        error_message: Option<&str>,
    ) -> Result<(), IntegrationError>;

    async fn list_matching_integrations(
        &self,
        organisation: &str,
        notification_type: &str,
    ) -> Result<Vec<Integration>, IntegrationError>;

    async fn list_deliveries(
        &self,
        integration_id: &str,
        limit: usize,
    ) -> Result<Vec<NotificationDelivery>, IntegrationError>;

    async fn update_integration_config(
        &self,
        organisation: &str,
        id: &str,
        name: &str,
        config: &IntegrationConfig,
    ) -> Result<(), IntegrationError>;

    async fn get_integration_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Integration, IntegrationError>;

    async fn get_slack_user_link(
        &self,
        user_id: &str,
        team_id: &str,
    ) -> Result<Option<SlackUserLink>, IntegrationError>;

    async fn upsert_slack_user_link(&self, link: &SlackUserLink) -> Result<(), IntegrationError>;

    async fn delete_slack_user_link(
        &self,
        user_id: &str,
        team_id: &str,
    ) -> Result<(), IntegrationError>;

    async fn list_slack_user_links(
        &self,
        user_id: &str,
    ) -> Result<Vec<SlackUserLink>, IntegrationError>;

    async fn get_slack_message_ref(
        &self,
        integration_id: &str,
        release_id: &str,
    ) -> Result<Option<SlackMessageRef>, IntegrationError>;

    async fn upsert_slack_message_ref(
        &self,
        msg_ref: &SlackMessageRef,
    ) -> Result<(), IntegrationError>;

    /// Serialises the Slack message work for one release across replicas.
    /// Hold the lock until the message and its ref are written, then
    /// `release` it.
    async fn lock_slack_release(
        &self,
        integration_id: &str,
        release_id: &str,
    ) -> Result<Box<dyn SlackReleaseLock>, IntegrationError>;

    /// Atomically records that a thread reply is about to be posted. Returns
    /// false when it already was (here or on another replica).
    async fn claim_slack_thread_key(
        &self,
        integration_id: &str,
        release_id: &str,
        key: &str,
    ) -> Result<bool, IntegrationError>;

    async fn release_slack_thread_key(
        &self,
        integration_id: &str,
        release_id: &str,
        key: &str,
    ) -> Result<(), IntegrationError>;

    async fn get_slack_emoji_set(
        &self,
        organisation: &str,
    ) -> Result<Option<slack_release::SlackEmojiSet>, IntegrationError>;

    async fn set_slack_emoji_set(
        &self,
        organisation: &str,
        set: &slack_release::SlackEmojiSet,
        updated_by: &str,
    ) -> Result<(), IntegrationError>;

    async fn get_slack_baseline(
        &self,
        integration_id: &str,
    ) -> Result<Option<baseline::SlackBaseline>, IntegrationError>;

    async fn upsert_slack_baseline(
        &self,
        baseline: &baseline::SlackBaseline,
    ) -> Result<(), IntegrationError>;

    async fn get_directory_match(
        &self,
        integration_id: &str,
        user_id: &str,
    ) -> Result<Option<baseline::DirectoryMatch>, IntegrationError>;

    async fn upsert_directory_match(
        &self,
        m: &baseline::DirectoryMatch,
    ) -> Result<(), IntegrationError>;

    async fn delete_directory_matches(&self, integration_id: &str) -> Result<(), IntegrationError>;

    /// Every integration of one type across all organisations; used only to
    /// resolve signed provider events to the integration they concern.
    async fn list_integrations_of_type(
        &self,
        integration_type: IntegrationType,
    ) -> Result<Vec<Integration>, IntegrationError>;

    /// Binds a GitHub installation to one Forest organisation. `Duplicate` if
    /// it is already bound to a different integration.
    async fn bind_github_installation(
        &self,
        installation_id: i64,
        organisation: &str,
        integration_id: &str,
    ) -> Result<(), IntegrationError>;

    /// The (organisation, integration id) a GitHub installation is bound to.
    async fn get_github_installation_binding(
        &self,
        installation_id: i64,
    ) -> Result<Option<(String, String)>, IntegrationError>;
}

pub fn generate_api_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let encoded = hex_encode(&bytes);
    format!("fgi_{encoded}")
}

pub fn hash_api_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(token.as_bytes());
    hex_encode(&hash)
}

fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// Rejects anything but HTTPS, except localhost for development.
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
    if name.chars().any(|c| c.is_control()) {
        return Err(IntegrationError::InvalidInput(
            "Integration name contains invalid characters".to_string(),
        ));
    }
    Ok(())
}

#[async_trait::async_trait]
pub trait SlackReleaseLock: Send {
    async fn release(self: Box<Self>);
}

struct InMemoryReleaseLock(#[allow(dead_code)] tokio::sync::OwnedMutexGuard<()>);

#[async_trait::async_trait]
impl SlackReleaseLock for InMemoryReleaseLock {
    async fn release(self: Box<Self>) {}
}

pub struct InMemoryIntegrationStore {
    integrations: std::sync::Mutex<Vec<Integration>>,
    rules: std::sync::Mutex<Vec<NotificationRule>>,
    deliveries: std::sync::Mutex<Vec<NotificationDelivery>>,
    token_hashes: std::sync::Mutex<HashMap<String, String>>,
    slack_user_links: std::sync::Mutex<Vec<SlackUserLink>>,
    slack_message_refs: std::sync::Mutex<Vec<SlackMessageRef>>,
    slack_baselines: std::sync::Mutex<HashMap<String, baseline::SlackBaseline>>,
    slack_emoji_sets: std::sync::Mutex<HashMap<String, slack_release::SlackEmojiSet>>,
    release_locks: std::sync::Mutex<HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    directory_matches: std::sync::Mutex<HashMap<(String, String), baseline::DirectoryMatch>>,
    github_installations: std::sync::Mutex<HashMap<i64, (String, String)>>,
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
            slack_baselines: std::sync::Mutex::new(HashMap::new()),
            slack_emoji_sets: std::sync::Mutex::new(HashMap::new()),
            release_locks: std::sync::Mutex::new(HashMap::new()),
            directory_matches: std::sync::Mutex::new(HashMap::new()),
            github_installations: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryIntegrationStore {
    fn default() -> Self {
        Self::new()
    }
}

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
        let stored = Integration { api_token: None, ..integration.clone() };
        store.push(stored);

        self.token_hashes.lock().unwrap().insert(token_hash, id.clone());

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
        let mut rules = self.rules.lock().unwrap();
        rules.retain(|r| r.integration_id != id);
        self.slack_baselines.lock().unwrap().remove(id);
        self.directory_matches
            .lock()
            .unwrap()
            .retain(|(iid, _), _| iid != id);
        self.github_installations
            .lock()
            .unwrap()
            .retain(|_, (_, iid)| iid != id);
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
            let thread_keys = if existing.message_ts == msg_ref.message_ts {
                std::mem::take(&mut existing.thread_keys)
            } else {
                Vec::new()
            };
            *existing = SlackMessageRef {
                thread_keys,
                ..msg_ref.clone()
            };
        } else {
            refs.push(SlackMessageRef {
                thread_keys: Vec::new(),
                ..msg_ref.clone()
            });
        }
        Ok(())
    }

    async fn lock_slack_release(
        &self,
        integration_id: &str,
        release_id: &str,
    ) -> Result<Box<dyn SlackReleaseLock>, IntegrationError> {
        let lock = {
            let mut locks = self.release_locks.lock().unwrap();
            locks.retain(|_, l| std::sync::Arc::strong_count(l) > 1);
            locks
                .entry(format!("{integration_id}/{release_id}"))
                .or_default()
                .clone()
        };
        Ok(Box::new(InMemoryReleaseLock(lock.lock_owned().await)))
    }

    async fn claim_slack_thread_key(
        &self,
        integration_id: &str,
        release_id: &str,
        key: &str,
    ) -> Result<bool, IntegrationError> {
        let mut refs = self.slack_message_refs.lock().unwrap();
        let Some(r) = refs
            .iter_mut()
            .find(|r| r.integration_id == integration_id && r.release_id == release_id)
        else {
            return Ok(false);
        };
        if r.thread_keys.iter().any(|k| k == key) {
            return Ok(false);
        }
        r.thread_keys.push(key.to_string());
        Ok(true)
    }

    async fn release_slack_thread_key(
        &self,
        integration_id: &str,
        release_id: &str,
        key: &str,
    ) -> Result<(), IntegrationError> {
        let mut refs = self.slack_message_refs.lock().unwrap();
        if let Some(r) = refs
            .iter_mut()
            .find(|r| r.integration_id == integration_id && r.release_id == release_id)
        {
            r.thread_keys.retain(|k| k != key);
        }
        Ok(())
    }

    async fn get_slack_emoji_set(
        &self,
        organisation: &str,
    ) -> Result<Option<slack_release::SlackEmojiSet>, IntegrationError> {
        Ok(self.slack_emoji_sets.lock().unwrap().get(organisation).cloned())
    }

    async fn set_slack_emoji_set(
        &self,
        organisation: &str,
        set: &slack_release::SlackEmojiSet,
        _updated_by: &str,
    ) -> Result<(), IntegrationError> {
        self.slack_emoji_sets
            .lock()
            .unwrap()
            .insert(organisation.to_string(), set.clone());
        Ok(())
    }

    async fn get_slack_baseline(
        &self,
        integration_id: &str,
    ) -> Result<Option<baseline::SlackBaseline>, IntegrationError> {
        Ok(self.slack_baselines.lock().unwrap().get(integration_id).cloned())
    }

    async fn upsert_slack_baseline(
        &self,
        b: &baseline::SlackBaseline,
    ) -> Result<(), IntegrationError> {
        self.slack_baselines
            .lock()
            .unwrap()
            .insert(b.integration_id.clone(), b.clone());
        Ok(())
    }

    async fn get_directory_match(
        &self,
        integration_id: &str,
        user_id: &str,
    ) -> Result<Option<baseline::DirectoryMatch>, IntegrationError> {
        Ok(self
            .directory_matches
            .lock()
            .unwrap()
            .get(&(integration_id.to_string(), user_id.to_string()))
            .cloned())
    }

    async fn upsert_directory_match(
        &self,
        m: &baseline::DirectoryMatch,
    ) -> Result<(), IntegrationError> {
        self.directory_matches
            .lock()
            .unwrap()
            .insert((m.integration_id.clone(), m.user_id.clone()), m.clone());
        Ok(())
    }

    async fn delete_directory_matches(&self, integration_id: &str) -> Result<(), IntegrationError> {
        self.directory_matches
            .lock()
            .unwrap()
            .retain(|(iid, _), _| iid != integration_id);
        Ok(())
    }

    async fn list_integrations_of_type(
        &self,
        integration_type: IntegrationType,
    ) -> Result<Vec<Integration>, IntegrationError> {
        Ok(self
            .integrations
            .lock()
            .unwrap()
            .iter()
            .filter(|i| i.integration_type == integration_type)
            .cloned()
            .collect())
    }

    async fn bind_github_installation(
        &self,
        installation_id: i64,
        organisation: &str,
        integration_id: &str,
    ) -> Result<(), IntegrationError> {
        let mut bindings = self.github_installations.lock().unwrap();
        if let Some((_, existing)) = bindings.get(&installation_id)
            && existing != integration_id
        {
            return Err(IntegrationError::Duplicate(
                "GitHub installation is already connected".into(),
            ));
        }
        bindings.insert(
            installation_id,
            (organisation.to_string(), integration_id.to_string()),
        );
        Ok(())
    }

    async fn get_github_installation_binding(
        &self,
        installation_id: i64,
    ) -> Result<Option<(String, String)>, IntegrationError> {
        Ok(self
            .github_installations
            .lock()
            .unwrap()
            .get(&installation_id)
            .cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integration_type_roundtrip() {
        for t in &[IntegrationType::Slack, IntegrationType::Webhook, IntegrationType::GitHub] {
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
            DeliveryStatus::Skipped,
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
        assert_eq!(h1.len(), 64);
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

        assert!(created.api_token.is_some());
        let token = created.api_token.unwrap();
        assert!(token.starts_with("fgi_"));

        let token_hash = hash_api_token(&token);
        let found = store.get_integration_by_token_hash(&token_hash).await.unwrap();
        assert_eq!(found.id, created.id);
        assert!(found.api_token.is_none());

        let listed = store.list_integrations("myorg").await.unwrap();
        assert!(listed[0].api_token.is_none());
    }
}
