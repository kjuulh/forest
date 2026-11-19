use crate::NavItem;
use crate::format::urlencode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OrgSection {
    #[default]
    General,
    Members,
    Integrations,
    Access,
    Visibility,
    Rules,
    Developers,
}

impl OrgSection {
    pub const ALL: [OrgSection; 7] = [
        OrgSection::General,
        OrgSection::Members,
        OrgSection::Integrations,
        OrgSection::Access,
        OrgSection::Visibility,
        OrgSection::Rules,
        OrgSection::Developers,
    ];

    pub fn label(self) -> &'static str {
        match self {
            OrgSection::General => "General",
            OrgSection::Members => "Members",
            OrgSection::Integrations => "Integrations",
            OrgSection::Access => "Access",
            OrgSection::Visibility => "Visibility",
            OrgSection::Rules => "Rules",
            OrgSection::Developers => "Developer settings",
        }
    }

    pub fn href(self, org: &str) -> String {
        let base = format!("/orgs/{}/settings", urlencode(org));
        match self {
            OrgSection::General => base,
            OrgSection::Members => format!("{base}/members"),
            OrgSection::Integrations => format!("{base}/integrations"),
            OrgSection::Access => format!("{base}/access"),
            OrgSection::Visibility => format!("{base}/visibility"),
            OrgSection::Rules => format!("{base}/rules"),
            OrgSection::Developers => format!("{base}/developers"),
        }
    }
}

pub fn nav_items(org: &str, current: OrgSection) -> Vec<NavItem> {
    OrgSection::ALL
        .iter()
        .map(|s| NavItem::new(s.label(), s.href(org), *s == current))
        .collect()
}
