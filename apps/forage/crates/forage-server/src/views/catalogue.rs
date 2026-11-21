use std::collections::BTreeMap;

use forage_core::registry::{ComponentSummary, ComponentVersionInfo, ToolShape};
use forage_ui::about::About;
use forage_ui::chrome::{Chrome, PublicNav, Tab};
use forage_ui::format::urlencode;
use sedge_rt::Trusted;

use crate::auth::Session;
use crate::manifest_view::{ManifestView, PlatformInfo};
use crate::state::AppState;
use crate::templates::TemplateEngine;
use crate::views::chrome::{Page, signed_in};

pub fn visitor(
    templates: &TemplateEngine,
    title: impl Into<String>,
    description: impl Into<String>,
    public: PublicNav,
) -> Chrome {
    Chrome {
        title: title.into(),
        description: description.into(),
        assets: templates.assets().clone(),
        build: templates.build_stamp().clone(),
        viewer: None,
        public,
    }
}

pub fn registry_chrome(
    state: &AppState,
    session: Option<&Session>,
    title: String,
    description: String,
) -> Chrome {
    match session {
        Some(session) => {
            let first_org = session.user.orgs.first().map(|o| o.name.clone());
            signed_in(
                state,
                session,
                Page {
                    title,
                    description,
                    org: first_org.as_deref(),
                    project: None,
                    projects: Vec::new(),
                    tab: Tab::Components,
                },
            )
        }
        None => visitor(&state.templates, title, description, PublicNav::default()),
    }
}

pub struct ComponentsPage {
    pub chrome: Chrome,
    pub query: String,
    pub filter_org: String,
    pub orgs: Vec<String>,
    pub components: Vec<ComponentSummary>,
    pub page: u32,
    pub total_pages: u32,
}

impl ComponentsPage {
    pub fn org_options(&self) -> Vec<(&str, bool)> {
        let mut options: Vec<(&str, bool)> = self
            .orgs
            .iter()
            .map(|o| (o.as_str(), *o == self.filter_org))
            .collect();
        if !self.filter_org.is_empty() && !self.orgs.contains(&self.filter_org) {
            options.push((self.filter_org.as_str(), true));
        }
        options
    }
}

pub struct OrgComponentsPage {
    pub chrome: Chrome,
    pub org: String,
    pub query: String,
    pub components: Vec<ComponentSummary>,
    pub page: u32,
    pub total_pages: u32,
}

pub struct ComponentDetailPage {
    pub chrome: Chrome,
    pub summary: ComponentSummary,
    pub versions: Vec<ComponentVersionInfo>,
    pub selected_version: Option<String>,
    pub description: String,
    pub readme: Option<Trusted>,
    pub manifest: Option<ManifestView>,
    pub manifest_html: Option<Trusted>,
    pub owners: Vec<String>,
    pub about: About,
    pub highlight_js: String,
}

impl ComponentDetailPage {
    pub fn is_tool(&self) -> bool {
        self.summary.shape.is_tool()
    }

    pub fn shown_version(&self) -> &str {
        self.selected_version
            .as_deref()
            .unwrap_or(&self.summary.latest_version)
    }

    pub fn is_selected(&self, version: &ComponentVersionInfo) -> bool {
        self.selected_version.as_deref() == Some(version.version.as_str())
    }

    pub fn component_ref(&self) -> String {
        format!("{}/{}", self.summary.organisation, self.summary.name)
    }

    pub fn pinned_ref(&self) -> String {
        match self.shown_version() {
            "" => self.component_ref(),
            version => format!("{}@{version}", self.component_ref()),
        }
    }

    pub fn methods_intro(&self) -> Option<bool> {
        if self.summary.methods.is_empty() {
            return None;
        }
        match self.summary.shape {
            ToolShape::Hybrid => Some(true),
            ToolShape::Component => Some(false),
            _ => None,
        }
    }

    pub fn env_defaults(&self) -> Option<&BTreeMap<String, String>> {
        self.manifest
            .as_ref()
            .map(|m| &m.include.env)
            .filter(|env| !env.is_empty())
    }

    pub fn platforms(&self) -> Option<&BTreeMap<String, PlatformInfo>> {
        self.manifest
            .as_ref()
            .map(|m| &m.platforms)
            .filter(|p| !p.is_empty())
    }

    pub fn upstream_url(&self) -> Option<&str> {
        self.manifest
            .as_ref()
            .and_then(|m| m.url.as_deref())
            .filter(|u| !u.is_empty())
    }

    pub fn shape_label(&self) -> Option<String> {
        match self.summary.shape {
            ToolShape::Component => None,
            shape => Some(shape_key(shape).replace('_', "-")),
        }
    }

    pub fn source_org_href(&self) -> String {
        format!("/orgs/{}/components", urlencode(&self.summary.organisation))
    }
}

pub fn shape_key(shape: ToolShape) -> &'static str {
    match shape {
        ToolShape::Unknown => "unknown",
        ToolShape::Component => "component",
        ToolShape::Hybrid => "hybrid",
        ToolShape::ToolBinary => "tool_binary",
        ToolShape::ToolExternal => "tool_external",
    }
}

pub fn opt(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.is_empty())
}

pub fn component_href(c: &ComponentSummary) -> String {
    format!(
        "/components/{}/{}",
        urlencode(&c.organisation),
        urlencode(&c.name)
    )
}

pub fn version_href(summary: &ComponentSummary, version: &str) -> String {
    format!("{}/{}", component_href(summary), urlencode(version))
}

pub fn versions_label(count: i32) -> String {
    if count == 1 {
        "1 version".into()
    } else {
        format!("{count} versions")
    }
}

pub fn org_components_href(org: &str) -> String {
    format!("/orgs/{}/components", urlencode(org))
}

pub struct OrgReleasesPage {
    pub chrome: Chrome,
    pub org: String,
    pub prefetch_js: String,
    pub graph_js: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageState {
    Reachable,
    Gap,
    Off,
    Unknown,
}

pub struct CoverageRow {
    pub username: String,
    pub role: String,
    pub state: CoverageState,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubHealth {
    Healthy,
    Uninstalled,
    Suspended,
    Unreachable,
    Unconfigured,
}

pub struct GithubConnectionPage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub integration_id: String,
    pub name: String,
    pub account_login: String,
    pub installed_by_login: String,
    pub health: GithubHealth,
    pub truncated: bool,
    pub rows: Vec<CoverageRow>,
}

pub struct IntegrationCoveragePage {
    pub chrome: Chrome,
    pub org: String,
    pub csrf_token: String,
    pub integration_id: String,
    pub name: String,
    pub enabled: bool,
    pub directory_mapping: bool,
    pub reachable: usize,
    pub truncated: bool,
    pub max_members: usize,
    pub rows: Vec<CoverageRow>,
}

impl IntegrationCoveragePage {
    pub fn not_reached(&self) -> usize {
        self.rows.len() - self.reachable
    }
}

pub fn integration_href(org: &str, id: &str) -> String {
    format!(
        "/orgs/{}/settings/integrations/{}",
        urlencode(org),
        urlencode(id)
    )
}

pub struct VisitorPage {
    pub chrome: Chrome,
}

pub struct ErrorPage {
    pub chrome: Chrome,
    pub status: u16,
    pub heading: String,
    pub message: String,
}

pub fn error_page(templates: &TemplateEngine, status: u16, heading: &str, message: &str) -> String {
    crate::views::pages::error::render(&ErrorPage {
        chrome: visitor(
            templates,
            format!("{heading} - Forest"),
            message,
            PublicNav::default(),
        ),
        status,
        heading: heading.into(),
        message: message.into(),
    })
}

pub struct MaintenancePage {
    pub css_href: String,
}
