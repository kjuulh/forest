#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AboutLinkKind {
    Git,
    Homepage,
    Docs,
    Support,
}

impl AboutLinkKind {
    pub fn icon(self) -> &'static str {
        match self {
            AboutLinkKind::Git => "github",
            AboutLinkKind::Homepage => "globe",
            AboutLinkKind::Docs => "book",
            AboutLinkKind::Support => "lifebuoy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AboutLink {
    pub kind: AboutLinkKind,
    pub href: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct About {
    pub links: Vec<AboutLink>,
    pub domain: String,
    pub owner: String,
}

impl About {
    pub fn has_facts(&self) -> bool {
        !self.domain.is_empty() || !self.owner.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.links.is_empty() && !self.has_facts()
    }
}
