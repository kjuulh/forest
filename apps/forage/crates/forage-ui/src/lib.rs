pub mod format;
pub mod templates;
mod tone;

pub use templates::components;
pub use tone::{AlertKind, Tone};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    #[default]
    Primary,
    Secondary,
    SmallNeutral,
    SmallPositive,
    SmallDanger,
}

impl ButtonVariant {
    pub fn classes(self) -> &'static str {
        match self {
            ButtonVariant::Primary => {
                "bg-gray-900 text-white px-4 py-2 rounded-md text-sm hover:bg-gray-800 transition-colors"
            }
            ButtonVariant::Secondary => "text-sm text-gray-500 hover:text-gray-900",
            ButtonVariant::SmallNeutral => {
                "text-xs px-2.5 py-1 rounded border border-gray-300 text-gray-600 hover:bg-gray-50"
            }
            ButtonVariant::SmallPositive => {
                "text-xs px-2.5 py-1 rounded border border-green-300 text-green-700 hover:bg-green-50"
            }
            ButtonVariant::SmallDanger => {
                "text-xs px-2.5 py-1 rounded border border-red-300 text-red-600 hover:bg-red-50"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    pub label: String,
    pub href: Option<String>,
}

impl Crumb {
    pub fn link(label: impl Into<String>, href: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: Some(href.into()),
        }
    }

    pub fn here(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavItem {
    pub label: String,
    pub href: String,
    pub current: bool,
}

impl NavItem {
    pub fn new(label: impl Into<String>, href: impl Into<String>, current: bool) -> Self {
        Self {
            label: label.into(),
            href: href.into(),
            current,
        }
    }
}

pub fn pagination_href(base_url: &str, params: &[(&str, &str)], page: u32) -> String {
    let mut href = format!("{base_url}?");
    for (k, v) in params {
        if !v.is_empty() {
            href.push_str(&format!(
                "{}={}&",
                format::urlencode(k),
                format::urlencode(v)
            ));
        }
    }
    href.push_str(&format!("page={page}"));
    href
}
