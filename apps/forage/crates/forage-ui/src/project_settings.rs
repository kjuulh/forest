use crate::Crumb;
use crate::format::urlencode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Triggers,
    Policies,
    Pipelines,
    Visibility,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Triggers,
        Section::Policies,
        Section::Pipelines,
        Section::Visibility,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Section::Triggers => "triggers",
            Section::Policies => "policies",
            Section::Pipelines => "pipelines",
            Section::Visibility => "visibility",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Section::Triggers => "Triggers",
            Section::Policies => "Policies",
            Section::Pipelines => "Pipelines",
            Section::Visibility => "Visibility",
        }
    }
}

pub fn crumbs(org: &str, project: &str, section: Section, crumb: Option<&str>) -> Vec<Crumb> {
    let base = format!("/orgs/{}/projects/{}", urlencode(org), urlencode(project));
    let mut crumbs = vec![
        Crumb::link(org, format!("/orgs/{}/projects", urlencode(org))),
        Crumb::link(project, base.clone()),
        Crumb::link("Settings", format!("{base}/settings")),
    ];
    match crumb {
        Some(c) => {
            crumbs.push(Crumb::link(
                section.label(),
                format!("{base}/settings/{}", section.slug()),
            ));
            crumbs.push(Crumb::here(c));
        }
        None => crumbs.push(Crumb::here(section.label())),
    }
    crumbs
}
