mod auth;
mod developers;
mod directory;
mod device;
mod events;
mod integrations;
mod introspect;
pub(crate) mod oauth;
mod pages;
mod platform;
mod registry;
mod visibility;

use axum::Router;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use forage_core::platform::validate_slug;
use forage_core::session::CachedOrg;
use minijinja::context;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(pages::router())
        .merge(auth::router())
        .merge(device::router())
        .merge(platform::router())
        .merge(events::router())
        .merge(integrations::router())
        .merge(developers::router())
        .merge(visibility::router())
        .merge(directory::routes())
        .merge(introspect::routes())
        .merge(oauth::router())
        .merge(registry::router())
}

fn error_page(state: &AppState, status: StatusCode, heading: &str, message: &str) -> Response {
    error_page_detail(state, status, heading, message, None)
}

fn error_page_detail(
    state: &AppState,
    status: StatusCode,
    heading: &str,
    message: &str,
    detail: Option<&str>,
) -> Response {
    let html = state.templates.render(
        "pages/error.html.jinja",
        context! {
            title => format!("{} - Forest", heading),
            description => message,
            status => status.as_u16(),
            heading => heading,
            message => message,
            detail => detail,
        },
    );
    match html {
        Ok(body) => (status, Html(body)).into_response(),
        Err(_) => status.into_response(),
    }
}

fn internal_error(state: &AppState, context: &str, err: &dyn std::fmt::Display) -> Response {
    let detail = format!("{err:#}");
    tracing::error!("{context}: {detail}");
    error_page_detail(
        state,
        StatusCode::INTERNAL_SERVER_ERROR,
        "Something went wrong",
        "An internal error occurred. Please try again.",
        Some(&detail),
    )
}

fn warn_default<T: Default>(context: &str, result: Result<T, impl std::fmt::Display>) -> T {
    match result {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("{context}: {e:#}");
            T::default()
        }
    }
}

pub(super) fn render_markdown(md: &str) -> String {
    use std::collections::{HashMap, HashSet};

    let mut opts = pulldown_cmark::Options::empty();
    opts.insert(pulldown_cmark::Options::ENABLE_TABLES);

    let parser = pulldown_cmark::Parser::new_ext(md, opts);
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, parser);

    let mut tag_attrs: HashMap<&str, HashSet<&str>> = HashMap::new();
    tag_attrs.insert("code", HashSet::from(["class"]));
    tag_attrs.insert("pre", HashSet::from(["class"]));

    ammonia::Builder::default()
        .tag_attributes(tag_attrs)
        .attribute_filter(|_element, attribute, value| {
            if attribute == "class" {
                let all_lang = value
                    .split_ascii_whitespace()
                    .all(|tok| tok.starts_with("language-"));
                if all_lang { Some(value.into()) } else { None }
            } else {
                Some(value.into())
            }
        })
        .clean(&html)
        .to_string()
}

fn orgs_context(orgs: &[CachedOrg]) -> Vec<minijinja::Value> {
    orgs.iter()
        .map(|o| context! { name => o.name, role => o.role })
        .collect()
}

#[allow(clippy::result_large_err)]
fn require_org_membership<'a>(
    state: &AppState,
    orgs: &'a [CachedOrg],
    org: &str,
) -> Result<&'a CachedOrg, Response> {
    if !validate_slug(org) {
        return Err(error_page(
            state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid organisation name.",
        ));
    }
    orgs.iter().find(|o| o.name == org).ok_or_else(|| {
        error_page(
            state,
            StatusCode::FORBIDDEN,
            "Access denied",
            "You don't have access to this organisation.",
        )
    })
}

#[cfg(test)]
mod markdown_tests {
    use super::render_markdown;

    #[test]
    fn gfm_table_renders_to_html_table() {
        let md = "\
| Method  | Description     |
| ------- | --------------- |
| init    | Bootstrap repo  |
| publish | Ship a version  |
";
        let html = render_markdown(md);
        assert!(html.contains("<table"), "expected <table> in: {html}");
        assert!(html.contains("<thead"), "expected <thead> in: {html}");
        assert!(html.contains("<tbody"), "expected <tbody> in: {html}");
        assert!(html.contains("<th>Method</th>"), "{html}");
        assert!(html.contains("<td>publish</td>"), "{html}");
    }

    #[test]
    fn fenced_code_class_survives_sanitisation() {
        let html = render_markdown("```rust\nlet x = 1;\n```\n");
        assert!(html.contains(r#"class="language-rust""#), "{html}");
    }

    #[test]
    fn arbitrary_class_token_dropped() {
        let html = render_markdown(r#"<div class="hidden">x</div>"#);
        assert!(!html.contains("class=\"hidden\""), "{html}");
    }
}
