use forage_ui::chrome::{Chrome, Tab, Viewer};

use crate::auth::Session;
use crate::state::AppState;

pub struct Page<'a> {
    pub title: String,
    pub description: String,
    pub org: Option<&'a str>,
    pub project: Option<&'a str>,
    pub projects: Vec<String>,
    pub tab: Tab,
}

pub fn signed_in(state: &AppState, session: &Session, page: Page<'_>) -> Chrome {
    Chrome {
        title: page.title,
        description: page.description,
        assets: state.templates.assets().clone(),
        build: state.templates.build_stamp().clone(),
        viewer: Some(Viewer {
            username: session.user.username.clone(),
            csrf_token: session.csrf_token.clone(),
            orgs: session.user.orgs.iter().map(|o| o.name.clone()).collect(),
            current_org: page.org.map(Into::into),
            projects: page.projects,
            project: page.project.map(Into::into),
            tab: page.tab,
        }),
        ..Default::default()
    }
}
