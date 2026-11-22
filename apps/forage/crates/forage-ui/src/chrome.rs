use crate::NavItem;
use crate::format::urlencode;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Chrome {
    pub title: String,
    pub description: String,
    pub assets: Assets,
    pub build: BuildStamp,
    pub viewer: Option<Viewer>,
    pub public: PublicNav,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assets {
    pub css_hash: String,
    pub ui_js_hash: String,
    pub components_js_hash: String,
    pub sedx_js_hash: String,
}

impl Default for Assets {
    fn default() -> Self {
        Self {
            css_hash: "dev".into(),
            ui_js_hash: "dev".into(),
            components_js_hash: "dev".into(),
            sedx_js_hash: "dev".into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildStamp {
    pub commit: String,
    pub time: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublicNav {
    pub is_landing: bool,
    pub is_auth_page: bool,
}

impl PublicNav {
    pub fn show_marketing_links(&self) -> bool {
        !self.is_landing && !self.is_auth_page
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Viewer {
    pub username: String,
    pub csrf_token: String,
    pub orgs: Vec<String>,
    pub current_org: Option<String>,
    pub projects: Vec<String>,
    pub project: Option<String>,
    pub tab: Tab,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    None,
    Dashboard,
    Projects,
    Components,
    Destinations,
    Settings,
    ProjectOverview,
    ProjectComponents,
    ProjectReleases,
    ProjectGraph,
    ProjectSettings,
}

impl Viewer {
    pub fn org_href(&self, org: &str) -> String {
        format!("/orgs/{}/projects", urlencode(org))
    }

    pub fn project_href(&self, project: &str) -> String {
        format!(
            "/orgs/{}/projects/{}",
            urlencode(self.current_org.as_deref().unwrap_or_default()),
            urlencode(project)
        )
    }

    pub fn notifications_href(&self) -> String {
        match &self.current_org {
            Some(org) => format!("/orgs/{}/notifications", urlencode(org)),
            None => "/notifications".into(),
        }
    }

    pub fn tabs(&self) -> Vec<NavItem> {
        let tab =
            |label: &str, href: String, which: Tab| NavItem::new(label, href, self.tab == which);
        match (&self.current_org, &self.project) {
            (Some(org), Some(project)) => {
                let base = format!("/orgs/{}/projects/{}", urlencode(org), urlencode(project));
                vec![
                    tab("Overview", base.clone(), Tab::ProjectOverview),
                    tab(
                        "Components",
                        format!("{base}/components"),
                        Tab::ProjectComponents,
                    ),
                    tab("Releases", format!("{base}/releases"), Tab::ProjectReleases),
                    tab("Graph", format!("{base}/graph"), Tab::ProjectGraph),
                    tab("Settings", format!("{base}/settings"), Tab::ProjectSettings),
                ]
            }
            (Some(org), None) => {
                let base = format!("/orgs/{}", urlencode(org));
                vec![
                    tab("Projects", format!("{base}/projects"), Tab::Projects),
                    tab("Components", format!("{base}/components"), Tab::Components),
                    tab(
                        "Destinations",
                        format!("{base}/destinations"),
                        Tab::Destinations,
                    ),
                    tab("Settings", format!("{base}/settings"), Tab::Settings),
                ]
            }
            _ => vec![tab("Overview", "/dashboard".into(), Tab::Dashboard)],
        }
    }
}
