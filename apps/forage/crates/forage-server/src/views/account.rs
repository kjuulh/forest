use forage_core::auth::{LinkedIdentity, LinkedProvider, PersonalAccessToken, UserEmail};
use forage_core::integrations::baseline::PersonalPreference;
use forage_core::platform::OAuthGrant;
use forage_ui::chrome::{Chrome, Tab};
use forage_ui::format::urlencode;
use sedge_rt::Trusted;

use crate::auth::Session;
use crate::state::AppState;
use crate::views::chrome::{Page, signed_in};

pub fn chrome(
    state: &AppState,
    session: &Session,
    org: Option<&str>,
    title: impl Into<String>,
    description: impl Into<String>,
) -> Chrome {
    signed_in(
        state,
        session,
        Page {
            title: title.into(),
            description: description.into(),
            org,
            project: None,
            projects: Vec::new(),
            tab: Tab::None,
        },
    )
}

pub fn first_org(session: &Session) -> Option<&str> {
    session.user.orgs.first().map(|o| o.name.as_str())
}

pub struct AccountPage {
    pub chrome: Chrome,
    pub csrf_token: String,
    pub username: String,
    pub profile_picture_url: Option<String>,
    pub emails: Vec<UserEmail>,
    pub mfa_enabled: bool,
    pub back_href: String,
    pub back_label: String,
    pub error: Option<String>,
    pub flash: Option<String>,
    pub oauth_error: Option<String>,
    pub linked: Option<LinkedAccounts>,
    pub slack_dm: Option<SlackDm>,
    pub cli_prefs: Vec<CliPreference>,
}

pub fn back_link(org: Option<&str>) -> (String, String) {
    match org {
        Some(org) => (
            format!("/orgs/{}/projects", urlencode(org)),
            org.to_string(),
        ),
        None => ("/dashboard".into(), "Dashboard".into()),
    }
}

pub struct LinkedAccounts {
    pub accounts: Vec<LinkedAccount>,
    pub link_github: bool,
    pub link_google: bool,
    pub add_slack: bool,
}

pub struct LinkedAccount {
    pub provider: LinkedProvider,
    pub title: String,
    pub detail: String,
    pub disconnect_key: String,
}

impl LinkedAccount {
    pub fn from_identity(identity: &LinkedIdentity) -> Self {
        let mut title = identity.provider.display_name().to_string();
        if let Some(subtitle) = identity.subtitle.as_deref().filter(|s| !s.is_empty()) {
            title.push_str(" · ");
            title.push_str(subtitle);
        }
        let mut detail = identity.display_name.clone();
        if let Some(email) = identity
            .email
            .as_deref()
            .filter(|e| !e.is_empty() && *e != identity.display_name)
        {
            detail.push_str(" · ");
            detail.push_str(email);
        }
        Self {
            provider: identity.provider,
            title,
            detail,
            disconnect_key: identity.disconnect_key.clone().unwrap_or_default(),
        }
    }

    pub fn provider_key(&self) -> &'static str {
        self.provider.as_str()
    }

    pub fn is_slack(&self) -> bool {
        self.provider == LinkedProvider::Slack
    }

    pub fn disconnect_action(&self) -> String {
        format!("/settings/account/{}/disconnect", self.provider.as_str())
    }

    pub fn disconnect_confirm(&self) -> String {
        format!("Disconnect this {} account?", self.provider.display_name())
    }
}

pub struct SlackDm {
    pub rows: Vec<DmRow>,
    pub routes: Vec<DmRoute>,
}

pub struct DmRow {
    pub key: String,
    pub label: String,
    pub mode: PersonalPreference,
    pub org_default: &'static str,
}

impl DmRow {
    pub fn is(&self, mode: PersonalPreference) -> bool {
        self.mode == mode
    }
}

pub struct DmChoice {
    pub enabled: &'static str,
    pub label: &'static str,
    pub mode: PersonalPreference,
}

pub const DM_CHOICES: [DmChoice; 2] = [
    DmChoice {
        enabled: "true",
        label: "On",
        mode: PersonalPreference::On,
    },
    DmChoice {
        enabled: "false",
        label: "Off",
        mode: PersonalPreference::Off,
    },
];

pub fn choice_classes(selected: bool) -> &'static str {
    if selected {
        "px-3 py-1 text-sm rounded-md border transition-colors bg-gray-900 text-white border-gray-900"
    } else {
        "px-3 py-1 text-sm rounded-md border transition-colors border-gray-300 hover:bg-gray-50"
    }
}

pub struct DmRoute {
    pub org: String,
    pub team_name: String,
    pub reachable: bool,
    pub label: String,
}

pub struct CliPreference {
    pub key: &'static str,
    pub label: &'static str,
    pub enabled: bool,
}

pub const CLI_CHANNEL: &str = "NOTIFICATION_CHANNEL_CLI";

const CLI_EVENTS: [(&str, &str); 4] = [
    ("NOTIFICATION_TYPE_RELEASE_ANNOTATED", "Release annotated"),
    ("NOTIFICATION_TYPE_RELEASE_STARTED", "Release started"),
    ("NOTIFICATION_TYPE_RELEASE_SUCCEEDED", "Release succeeded"),
    ("NOTIFICATION_TYPE_RELEASE_FAILED", "Release failed"),
];

impl CliPreference {
    pub fn all(prefs: &[forage_core::platform::NotificationPreference]) -> Vec<Self> {
        CLI_EVENTS
            .iter()
            .map(|(key, label)| Self {
                key,
                label,
                enabled: prefs
                    .iter()
                    .any(|p| p.enabled && p.notification_type == *key && p.channel == CLI_CHANNEL),
            })
            .collect()
    }

    pub fn next_value(&self) -> &'static str {
        if self.enabled { "false" } else { "true" }
    }

    pub fn title(&self) -> String {
        format!(
            "{} {} via CLI",
            if self.enabled { "Disable" } else { "Enable" },
            self.label.to_lowercase()
        )
    }
}

pub struct TokensPage {
    pub chrome: Chrome,
    pub csrf_token: String,
    pub created_token: Option<String>,
    pub tokens: Vec<PersonalAccessToken>,
}

pub fn token_delete_action(token: &PersonalAccessToken) -> String {
    format!("/settings/tokens/{}/delete", urlencode(&token.token_id))
}

pub fn present(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.is_empty())
}

pub struct AuthorizedAppsPage {
    pub chrome: Chrome,
    pub csrf_token: String,
    pub grants: Vec<OAuthGrant>,
}

pub fn revoke_action(grant: &OAuthGrant) -> String {
    format!(
        "/settings/authorized-apps/{}/revoke",
        urlencode(&grant.app_id)
    )
}

pub fn revoke_confirm(grant: &OAuthGrant) -> String {
    format!(
        "Revoke access for {}? It will no longer be able to act on your behalf.",
        grant.name
    )
}

pub struct MfaSetupPage {
    pub chrome: Chrome,
    pub csrf_token: String,
    pub mfa_id: String,
    pub secret: String,
    pub qr_svg: Option<Trusted>,
}

pub struct NotificationsPage {
    pub chrome: Chrome,
    pub releases: Vec<NotificationRelease>,
    pub script: String,
}

pub struct NotificationsListPartial {
    pub releases: Vec<NotificationRelease>,
}

pub struct NotificationRelease {
    pub org: String,
    pub project: String,
    pub slug: String,
    pub title: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub branch: Option<String>,
    pub commit_sha: Option<String>,
    pub commit_message: Option<String>,
    pub source_user: Option<String>,
    pub created_at: String,
    pub summary_status: String,
    pub env_groups: Vec<EnvGroup>,
    pub pipeline_stages: Vec<NotificationStage>,
    pub has_pipeline: bool,
    pub destinations: Vec<NotificationDestination>,
}

impl NotificationRelease {
    pub fn href(&self) -> String {
        format!(
            "/orgs/{}/projects/{}/releases/{}",
            urlencode(&self.org),
            urlencode(&self.project),
            urlencode(&self.slug)
        )
    }

    pub fn project_href(&self) -> String {
        format!(
            "/orgs/{}/projects/{}",
            urlencode(&self.org),
            urlencode(&self.project)
        )
    }

    pub fn headline(&self) -> &str {
        present(&self.commit_message).unwrap_or(&self.title)
    }

    pub fn status_dot(&self) -> &'static str {
        match self.summary_status.as_str() {
            "RUNNING" => "bg-yellow-400 animate-pulse",
            "QUEUED" => "bg-blue-400",
            "FAILED" => "bg-red-500",
            "SUCCEEDED" => "bg-green-500",
            "TIMED_OUT" => "bg-orange-400",
            "CANCELLED" => "bg-gray-400",
            _ => "bg-gray-300",
        }
    }

    pub fn open(&self) -> bool {
        matches!(
            self.summary_status.as_str(),
            "RUNNING" | "QUEUED" | "FAILED"
        )
    }

    pub fn has_stages(&self) -> bool {
        self.has_pipeline && !self.pipeline_stages.is_empty()
    }

    pub fn stages_done(&self) -> String {
        let done = self
            .pipeline_stages
            .iter()
            .filter(|s| s.status == "SUCCEEDED")
            .count();
        format!("{done}/{}", self.pipeline_stages.len())
    }

    pub fn all_deployed(&self) -> bool {
        !self.env_groups.is_empty() && self.env_groups.iter().all(|g| g.status == "SUCCEEDED")
    }
}

pub struct EnvGroup {
    pub status: &'static str,
    pub envs: Vec<String>,
}

pub struct NotificationStage {
    pub stage_type: String,
    pub environment: String,
    pub status: String,
    pub duration_seconds: Option<i64>,
    pub started_at: Option<String>,
}

impl NotificationStage {
    pub fn deploy_dot(&self) -> &'static str {
        match self.status.as_str() {
            "SUCCEEDED" => "bg-green-500",
            "RUNNING" => "bg-yellow-500 animate-pulse",
            "QUEUED" => "bg-blue-400",
            "FAILED" => "bg-red-500",
            _ => "bg-gray-300",
        }
    }

    pub fn row_classes(&self, last: bool) -> String {
        let mut classes = String::from("px-4 py-2.5 flex items-center gap-3 text-sm");
        if !last {
            classes.push_str(" border-b border-gray-50");
        }
        if self.status == "PENDING" {
            classes.push_str(" opacity-50");
        }
        classes
    }

    pub fn deploy_tone(&self) -> &'static str {
        match self.status.as_str() {
            "SUCCEEDED" => "text-sm text-gray-700",
            "RUNNING" => "text-sm text-yellow-700",
            "FAILED" => "text-sm text-red-700",
            _ => "text-sm text-gray-400",
        }
    }

    pub fn deploy_label(&self) -> &'static str {
        match self.status.as_str() {
            "SUCCEEDED" => "Deployed to",
            "RUNNING" => "Deploying to",
            "QUEUED" => "Queued for",
            "FAILED" => "Failed on",
            "TIMED_OUT" => "Timed out on",
            "CANCELLED" => "Cancelled",
            _ => "Deploy to",
        }
    }

    pub fn wait_tone(&self) -> &'static str {
        match self.status.as_str() {
            "SUCCEEDED" => "text-sm text-gray-700",
            "RUNNING" => "text-sm text-yellow-700",
            _ => "text-sm text-gray-400",
        }
    }

    pub fn wait_label(&self) -> String {
        let label = match self.status.as_str() {
            "SUCCEEDED" => "Waited",
            "RUNNING" => "Waiting",
            "FAILED" => "Wait failed",
            "CANCELLED" => "Wait cancelled",
            _ => "Wait",
        };
        match self.duration_seconds.filter(|d| *d != 0) {
            Some(d) => format!("{label} {d}s"),
            None => label.to_string(),
        }
    }
}

pub struct NotificationDestination {
    pub name: String,
    pub environment: String,
    pub status: String,
    pub completed_at: Option<String>,
    pub queue_position: Option<i32>,
}

impl NotificationDestination {
    pub fn row_classes(&self, last: bool) -> &'static str {
        if last {
            "px-4 py-2 flex items-center gap-3 text-sm border-t border-gray-100"
        } else {
            "px-4 py-2 flex items-center gap-3 text-sm border-b border-gray-50 border-t border-gray-100"
        }
    }

    pub fn queued_label(&self) -> String {
        match self.queue_position.filter(|p| *p != 0) {
            Some(p) => format!("Queued #{p}"),
            None => "Queued".into(),
        }
    }
}

pub struct UserProfilePage {
    pub chrome: Chrome,
    pub username: String,
    pub profile_picture_url: Option<String>,
    pub created_at: Option<String>,
    pub heatmap: Heatmap,
    pub recent_releases: Vec<ProfileRelease>,
    pub contributed_projects: Vec<ContributedProject>,
}

pub struct Heatmap {
    pub total: u32,
    pub weeks: Vec<HeatmapWeek>,
}

impl Heatmap {
    pub fn heading(&self) -> String {
        format!(
            "{} contribution{} in the last year",
            self.total,
            if self.total == 1 { "" } else { "s" }
        )
    }
}

pub struct HeatmapWeek {
    pub month: Option<String>,
    pub days: Vec<HeatmapDay>,
}

pub struct HeatmapDay {
    pub date: String,
    pub count: u32,
    pub level: u8,
    pub in_range: bool,
}

impl HeatmapDay {
    pub fn title(&self) -> String {
        format!(
            "{}: {} contribution{}",
            self.date,
            self.count,
            if self.count == 1 { "" } else { "s" }
        )
    }
}

pub fn contribution_level(count: u32, max_count: u32) -> u8 {
    if count == 0 || max_count == 0 {
        return 0;
    }
    let ratio = (count as f64).sqrt() / (max_count as f64).sqrt();
    let opacity = (0.15 + ratio * 0.85).clamp(0.15, 1.0);
    ((opacity * 4.0).ceil() as u8).clamp(1, 4)
}

pub struct ProfileRelease {
    pub org: String,
    pub project: String,
    pub version: Option<String>,
    pub branch: Option<String>,
    pub commit_sha: Option<String>,
    pub created_at: String,
}

impl ProfileRelease {
    pub fn project_href(&self) -> String {
        format!(
            "/orgs/{}/projects/{}",
            urlencode(&self.org),
            urlencode(&self.project)
        )
    }
}

pub struct ContributedProject {
    pub org: String,
    pub project: String,
    pub release_count: u32,
}

impl ContributedProject {
    pub fn href(&self) -> String {
        format!(
            "/orgs/{}/projects/{}",
            urlencode(&self.org),
            urlencode(&self.project)
        )
    }

    pub fn releases_label(&self) -> String {
        format!(
            "{} release{}",
            self.release_count,
            if self.release_count == 1 { "" } else { "s" }
        )
    }
}

pub fn user_href(username: &str) -> String {
    format!("/users/{}", urlencode(username))
}
