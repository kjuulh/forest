use forage_core::compute::{ComputeInstance, Rollout};
use forage_core::platform::{AllowedDomain, OAuthApp, OrgMember, OrgRuleSet, ProjectMetadata};
use forage_ui::chrome::Chrome;
use forage_ui::format::urlencode;
use serde::Serialize;

pub struct MembersPage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub is_admin: bool,
    pub members: Vec<OrgMember>,
}

pub struct AccessPage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub is_admin: bool,
    pub flash_error: Option<String>,
    pub flash_info: Option<String>,
    pub domains: Vec<AllowedDomain>,
}

pub struct OrgVisibilityPage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub org_public: bool,
    pub can_change: bool,
    pub flash_error: Option<String>,
}

pub struct ProjectVisibilityPage {
    pub chrome: Chrome,
    pub org: String,
    pub project: String,
    pub csrf_token: String,
    pub org_public: bool,
    pub project_public: bool,
    pub component_public: bool,
    pub can_change: bool,
    pub flash_error: Option<String>,
}

pub struct UsagePage {
    pub chrome: Chrome,
    pub org: String,
    pub project_count: usize,
}

pub struct ComputePage {
    pub chrome: Chrome,
    pub org: String,
    pub instances: Vec<ComputeInstance>,
    pub rollouts: Vec<Rollout>,
}

pub struct RolloutPage {
    pub chrome: Chrome,
    pub org: String,
    pub rollout: Rollout,
    pub labels: Vec<(String, String)>,
}

impl RolloutPage {
    pub fn new(chrome: Chrome, org: String, rollout: Rollout) -> Self {
        let mut labels: Vec<(String, String)> = rollout
            .labels
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        labels.sort();
        Self {
            chrome,
            org,
            rollout,
            labels,
        }
    }
}

pub struct DevelopersPage {
    pub chrome: Chrome,
    pub org: String,
    pub error: Option<String>,
    pub apps: Vec<OAuthApp>,
}

pub struct DeveloperAppNewPage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub error: Option<String>,
}

pub struct DeveloperAppPage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub app: OAuthApp,
    pub client_secret: Option<String>,
    pub error: Option<String>,
}

pub struct OAuthOption {
    pub field: &'static str,
    pub value: &'static str,
    pub label: &'static str,
    pub help: &'static str,
}

pub const SCOPES: [OAuthOption; 7] = [
    OAuthOption {
        field: "scope_openid",
        value: "openid",
        label: "openid",
        help: "OpenID Connect sign-in (issues an ID token)",
    },
    OAuthOption {
        field: "scope_profile",
        value: "profile",
        label: "profile",
        help: "username, user ID and avatar",
    },
    OAuthOption {
        field: "scope_email",
        value: "email",
        label: "email",
        help: "verified email addresses",
    },
    OAuthOption {
        field: "scope_components_publish",
        value: "components:publish",
        label: "components:publish",
        help: "publish Forest components from CI",
    },
    OAuthOption {
        field: "scope_tokens_introspect",
        value: "tokens:introspect",
        label: "tokens:introspect",
        help: "check other apps’ tokens (a service that accepts them)",
    },
    OAuthOption {
        field: "scope_datagateway_apply",
        value: "datagateway:apply",
        label: "datagateway:apply",
        help: "define data gateway project metrics",
    },
    OAuthOption {
        field: "scope_datagateway_read",
        value: "datagateway:read",
        label: "datagateway:read",
        help: "query data gateway project metrics",
    },
];

pub const GRANTS: [OAuthOption; 3] = [
    OAuthOption {
        field: "grant_authorization_code",
        value: "authorization_code",
        label: "authorization_code",
        help: "“Sign in with Forest”, on behalf of a user",
    },
    OAuthOption {
        field: "grant_client_credentials",
        value: "client_credentials",
        label: "client_credentials",
        help: "a service calling as itself, with no user involved",
    },
    OAuthOption {
        field: "grant_device_code",
        value: "urn:ietf:params:oauth:grant-type:device_code",
        label: "device_code",
        help: "a CLI signs a user in with a code they approve on another device (RFC 8628)",
    },
];

pub const NEW_APP_GRANTS: usize = 2;

pub fn checked_by_default(option: &OAuthOption) -> bool {
    matches!(option.value, "profile" | "authorization_code")
}

#[derive(Serialize)]
pub struct RuleProject {
    pub name: String,
    pub metadata: ProjectMetadata,
}

pub struct OrgRulesPage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub is_admin: bool,
    pub rules: Vec<RuleRow>,
    pub rule_sets: Vec<OrgRuleSet>,
    pub projects: Vec<RuleProject>,
    pub policy_example: String,
    pub trigger_example: String,
    pub pipeline_example: String,
    pub pipeline_builder_js: String,
    pub org_rules_js: String,
}

pub struct RuleRow {
    pub name: String,
    pub enabled: bool,
    pub include_projects: Vec<String>,
    pub exclude_projects: Vec<String>,
    pub name_regex: Option<String>,
    pub metadata_match: Vec<(String, String)>,
    pub tags: Vec<String>,
    pub policy_count: usize,
    pub trigger_count: usize,
    pub pipeline_count: usize,
    pub policies_json: String,
    pub triggers_json: String,
    pub pipelines_json: String,
}

impl RuleRow {
    pub fn selects_everything(&self) -> bool {
        self.name_regex.is_none()
            && self.tags.is_empty()
            && self.include_projects.is_empty()
            && self.metadata_match.is_empty()
    }
}

impl From<&OrgRuleSet> for RuleRow {
    fn from(rule: &OrgRuleSet) -> Self {
        let selector = &rule.selector;
        let pretty = |v: Result<String, serde_json::Error>| v.unwrap_or_else(|_| "[]".into());
        Self {
            name: rule.name.clone(),
            enabled: rule.enabled,
            include_projects: selector.include_projects.clone(),
            exclude_projects: selector.exclude_projects.clone(),
            name_regex: selector.name_regex.clone().filter(|r| !r.is_empty()),
            metadata_match: selector
                .metadata_match
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            tags: selector.tags.clone(),
            policy_count: rule.policies.len(),
            trigger_count: rule.triggers.len(),
            pipeline_count: rule.release_pipelines.len(),
            policies_json: pretty(serde_json::to_string_pretty(&rule.policies)),
            triggers_json: pretty(serde_json::to_string_pretty(&rule.triggers)),
            pipelines_json: pretty(serde_json::to_string_pretty(&rule.release_pipelines)),
        }
    }
}

pub fn settings_href(org: &str, rest: &str) -> String {
    format!("/orgs/{}/settings{rest}", urlencode(org))
}

pub fn member_href(org: &str, user_id: &str, action: &str) -> String {
    settings_href(org, &format!("/members/{}/{action}", urlencode(user_id)))
}

pub fn developer_app_href(org: &str, app_id: &str) -> String {
    settings_href(org, &format!("/developers/{}", urlencode(app_id)))
}

pub fn rollout_href(org: &str, id: &str) -> String {
    settings_href(org, &format!("/compute/rollouts/{}", urlencode(id)))
}

pub fn rule_delete_href(org: &str, name: &str) -> String {
    settings_href(org, &format!("/rules/{}/delete", urlencode(name)))
}

pub fn resource_count(rollout: &Rollout) -> String {
    let n = rollout.resources.len();
    format!("{n} resource{}", if n == 1 { "" } else { "s" })
}
