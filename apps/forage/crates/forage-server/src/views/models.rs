use forage_core::platform::Trigger;
use forage_ui::chrome::Chrome;
use forage_ui::format::urlencode;

pub struct TriggersPage {
    pub chrome: Chrome,
    pub org: String,
    pub project: String,
    pub csrf_token: String,
    pub is_admin: bool,
    pub triggers: Vec<Trigger>,
    pub environments: Vec<String>,
    pub has_pipelines: bool,
    pub trigger_form_js: String,
}

pub struct TriggerEditPage {
    pub chrome: Chrome,
    pub org: String,
    pub project: String,
    pub csrf_token: String,
    pub trigger: Trigger,
    pub environments: Vec<String>,
    pub has_pipelines: bool,
}

pub fn pattern(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or_default()
}

pub fn project_href(org: &str, project: &str) -> String {
    format!("/orgs/{}/projects/{}", urlencode(org), urlencode(project))
}

pub fn trigger_href(org: &str, project: &str, id: &str) -> String {
    format!("{}/settings/triggers/{}", project_href(org, project), urlencode(id))
}

pub fn triggers_href(org: &str, project: &str) -> String {
    format!("{}/settings/triggers", project_href(org, project))
}

pub fn destinations_href(org: &str) -> String {
    format!("/orgs/{}/destinations", urlencode(org))
}

pub struct ProjectReleasesPage {
    pub chrome: Chrome,
    pub org: String,
    pub project: String,
    pub csrf_token: String,
    pub username: String,
    pub role: String,
    pub prefetch_js: String,
}

pub fn settings_href(org: &str, project: &str, section: &str) -> String {
    format!("{}/settings/{section}", project_href(org, project))
}
