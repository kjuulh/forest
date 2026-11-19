use forage_core::integrations::{IntegrationType, NotificationDelivery};
use forage_ui::chrome::{Chrome, Tab};
use forage_ui::format::urlencode;

use crate::auth::Session;
use crate::state::AppState;
use crate::views::chrome::{Page, signed_in};

pub fn chrome(
    state: &AppState,
    session: &Session,
    org: &str,
    title: String,
    description: String,
) -> Chrome {
    signed_in(
        state,
        session,
        Page {
            title,
            description,
            org: Some(org),
            project: None,
            projects: Vec::new(),
            tab: Tab::Settings,
        },
    )
}

pub struct OrgSettingsPage {
    pub chrome: Chrome,
    pub org: String,
    pub role: String,
}

pub struct IntegrationsPage {
    pub chrome: Chrome,
    pub org: String,
    pub error: Option<String>,
    pub integrations: Vec<IntegrationRow>,
}

pub struct IntegrationRow {
    pub id: String,
    pub name: String,
    pub kind: IntegrationType,
    pub enabled: bool,
    pub enabled_rules: usize,
    pub total_rules: usize,
}

impl IntegrationRow {
    pub fn events(&self) -> String {
        if self.kind == IntegrationType::GitHub {
            "Organisation connection".into()
        } else if self.enabled_rules == self.total_rules {
            "All events".into()
        } else if self.enabled_rules == 0 {
            "No events".into()
        } else {
            format!("{}/{} events", self.enabled_rules, self.total_rules)
        }
    }
}

pub struct IntegrationDetailPage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub id: String,
    pub name: String,
    pub kind: IntegrationType,
    pub enabled: bool,
    pub config: ConfigView,
    pub can_reinstall: bool,
    pub rules: Vec<RuleView>,
    pub deliveries: Vec<NotificationDelivery>,
    pub test_sent: bool,
    pub baseline: Option<BaselineView>,
    pub emoji: Vec<EmojiField>,
    pub emoji_saved: bool,
    pub saved: bool,
    pub error: Option<String>,
}

pub enum ConfigView {
    Slack { detail: String },
    Webhook { url: String, has_secret: bool },
}

pub struct RuleView {
    pub notification_type: String,
    pub label: String,
    pub enabled: bool,
}

pub struct BaselineView {
    pub sent_by: Option<SentBy>,
    pub directory_mapping: bool,
    pub updated_by: String,
    pub updated_at: String,
    pub defaults: Vec<RuleView>,
}

pub struct SentBy {
    pub id: String,
    pub name: String,
}

pub struct EmojiField {
    pub field: &'static str,
    pub fallback: &'static str,
    pub label: &'static str,
    pub value: String,
}

pub struct InstallWebhookPage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub error: Option<String>,
}

pub struct InstallSlackPage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub error: Option<String>,
    pub has_slack_oauth: bool,
}

pub struct InstallGithubPage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub error: Option<String>,
    pub has_github_app: bool,
}

pub struct IntegrationInstalledPage {
    pub chrome: Chrome,
    pub org: String,
    pub id: String,
    pub name: String,
    pub type_display: &'static str,
    pub api_token: Option<String>,
}

pub fn integrations_href(org: &str) -> String {
    format!("/orgs/{}/settings/integrations", urlencode(org))
}

pub fn integration_href(org: &str, id: &str) -> String {
    format!("{}/{}", integrations_href(org), urlencode(id))
}

pub fn message(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|m| !m.is_empty())
}

pub fn prefix(value: &str, chars: usize) -> &str {
    match value.char_indices().nth(chars) {
        Some((end, _)) => &value[..end],
        None => value,
    }
}

pub fn short_id(id: &str) -> (&str, bool) {
    let short = prefix(id, 12);
    (short, short.len() < id.len())
}

pub fn attempted_at(value: &str) -> String {
    prefix(value, 19).replace('T', " ")
}
