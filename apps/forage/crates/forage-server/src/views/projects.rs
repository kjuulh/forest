use forage_core::platform::{JoinOffer, ProjectMetadata, prettify_url};
use forage_core::registry::{ComponentSummary, ComponentVersionInfo};
use forage_ui::about::{About, AboutLink, AboutLinkKind};
use forage_ui::chrome::Chrome;
use forage_ui::format::urlencode;
use sedge_rt::Trusted;

pub struct DashboardPage {
    pub chrome: Chrome,
    pub csrf_token: String,
    pub join_offers: Vec<JoinOffer>,
    pub orgs: Vec<OrgCard>,
    pub recent_activity: Vec<ActivityItem>,
}

pub struct OrgCard {
    pub name: String,
    pub role: String,
}

impl OrgCard {
    pub fn initial(&self) -> String {
        self.name
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default()
    }

    pub fn href(&self) -> String {
        format!("/orgs/{}/projects", urlencode(&self.name))
    }
}

pub struct ActivityItem {
    pub org: String,
    pub project: String,
    pub slug: String,
    pub title: String,
    pub created_at: String,
    pub dest_envs: Vec<String>,
}

impl ActivityItem {
    pub fn href(&self) -> String {
        format!(
            "{}/releases/{}",
            project_href(&self.org, &self.project),
            urlencode(&self.slug)
        )
    }
}

pub struct OnboardingPage {
    pub chrome: Chrome,
    pub csrf_token: String,
    pub join_offers: Vec<JoinOffer>,
    pub error: Option<String>,
}

pub struct ProjectsPage {
    pub chrome: Chrome,
    pub org: String,
    pub projects: Vec<String>,
    pub filter_js: String,
}

pub struct ProjectDetailPage {
    pub chrome: Chrome,
    pub org: String,
    pub project: String,
    pub csrf_token: String,
    pub username: String,
    pub role: String,
    pub description: String,
    pub summary: Option<ComponentSummary>,
    pub versions: Vec<ComponentVersionInfo>,
    pub readme: Option<Trusted>,
    pub about: About,
    pub has_timeline: bool,
    pub prefetch_js: String,
    pub highlight_js: String,
    pub is_tool: bool,
    pub show_get_started: bool,
}

pub fn shows_metadata(summary: &ComponentSummary) -> bool {
    !summary.created_at.is_empty()
        || !summary.updated_at.is_empty()
        || !summary.contracts.is_empty()
}

pub fn component_ref(summary: &ComponentSummary) -> String {
    format!("{}/{}", summary.organisation, summary.name)
}

pub fn pinned_ref(summary: &ComponentSummary) -> String {
    if summary.latest_version.is_empty() {
        component_ref(summary)
    } else {
        format!("{}@{}", component_ref(summary), summary.latest_version)
    }
}

pub struct ProjectComponentsPage {
    pub chrome: Chrome,
    pub org: String,
    pub project: String,
    pub versions: Vec<ComponentVersionInfo>,
}

pub fn project_href(org: &str, project: &str) -> String {
    format!("/orgs/{}/projects/{}", urlencode(org), urlencode(project))
}

pub fn version_href(org: &str, project: &str, version: &str) -> String {
    format!(
        "/components/{}/{}/{}",
        urlencode(org),
        urlencode(project),
        urlencode(version)
    )
}

pub fn about(metadata: &ProjectMetadata) -> About {
    let links = [
        (AboutLinkKind::Git, &metadata.git_url),
        (AboutLinkKind::Homepage, &metadata.homepage),
        (AboutLinkKind::Docs, &metadata.docs_url),
        (AboutLinkKind::Support, &metadata.support_url),
    ]
    .into_iter()
    .filter(|(_, href)| !href.is_empty())
    .map(|(kind, href)| AboutLink {
        kind,
        href: href.clone(),
        label: prettify_url(href),
    })
    .collect();
    About {
        links,
        domain: metadata.domain.clone(),
        owner: metadata.owner.clone(),
    }
}
