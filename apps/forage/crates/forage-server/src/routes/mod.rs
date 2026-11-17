mod auth;
mod developers;
mod directory;
mod device;
mod events;
mod integrations;
mod introspect;
mod org_connections;
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
        .merge(org_connections::router())
        .merge(developers::router())
        .merge(visibility::router())
        .merge(directory::routes())
        .merge(introspect::routes())
        .merge(oauth::router())
        .merge(registry::router())
}

fn error_page(state: &AppState, status: StatusCode, heading: &str, message: &str) -> Response {
    let html = state.templates.render(
        "pages/error.html.jinja",
        context! {
            title => format!("{} - Forest", heading),
            description => message,
            status => status.as_u16(),
            heading => heading,
            message => message,
        },
    );
    match html {
        Ok(body) => (status, Html(body)).into_response(),
        Err(_) => status.into_response(),
    }
}

fn report_error(context: &str, err: &dyn std::fmt::Display) -> String {
    generic_error_message(record_error(context, err))
}

fn record_error(context: &str, err: &dyn std::fmt::Display) -> uuid::Uuid {
    let error_id = uuid::Uuid::new_v4();
    let diagnostic = redact_diagnostic(format!("{err:#}"));
    tracing::error!(%error_id, context, error = %diagnostic, "browser operation failed");
    error_id
}

fn generic_error_message(error_id: uuid::Uuid) -> String {
    format!("An internal error occurred. Please try again. Reference: {error_id}")
}

fn internal_error(state: &AppState, context: &str, err: &dyn std::fmt::Display) -> Response {
    let error_id = record_error(context, err);
    let message = generic_error_message(error_id);
    let mut response = error_page(
        state,
        StatusCode::INTERNAL_SERVER_ERROR,
        "Something went wrong",
        &message,
    );
    let mut encoded = uuid::Uuid::encode_buffer();
    response.headers_mut().insert(
        "x-error-id",
        error_id
            .hyphenated()
            .encode_lower(&mut encoded)
            .parse()
            .expect("server-minted UUID header"),
    );
    response
}

fn redact_diagnostic(mut diagnostic: String) -> String {
    use regex::Regex;
    use std::{borrow::Cow, sync::LazyLock};

    static URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)\b(?:https?|postgres(?:ql)?|mysql|rediss?|amqps?)://[^\s<>"']+"#)
            .expect("URL redaction regex")
    });
    static CREDENTIALS: LazyLock<[(Regex, &'static str); 3]> = LazyLock::new(|| {
        [
            (
                Regex::new(r"(?i)\b(?:bearer|basic)\s+[A-Za-z0-9._~+/=-]+")
                    .expect("authorization redaction regex"),
                "[REDACTED AUTH]",
            ),
            (
                Regex::new(r#"(?i)\b([a-z0-9_-]*(?:password|passwd|pwd|token|secret|(?:api|access)[_-]?key(?:[_-]?id)?)|authorization|cookie|set-cookie)\b(?:\\?["'])?\s*[:=]\s*(?:\\"(?:\\.|[^"\\])*\\"|"(?:\\.|[^"\\])*"|'[^']*'|[^\s,;}\]]+)"#)
                    .expect("credential redaction regex"),
                "${1}=[REDACTED]",
            ),
            (
                Regex::new(r"\b(?:eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+|(?:gh[pousr]_|github_pat_|forest_(?:pat|oat)_|xox[baprs]-)[A-Za-z0-9_-]+)")
                    .expect("token redaction regex"),
                "[REDACTED TOKEN]",
            ),
        ]
    });
    if let Cow::Owned(redacted) =
        URL.replace_all(
            &diagnostic,
            |captures: &regex::Captures<'_>| match reqwest::Url::parse(&captures[0]) {
                Ok(url) => match (url.host_str(), url.port()) {
                    (Some(host), Some(port)) => {
                        format!("{}://{host}:{port}/[REDACTED]", url.scheme())
                    }
                    (Some(host), None) => format!("{}://{host}/[REDACTED]", url.scheme()),
                    _ => "[REDACTED URL]".into(),
                },
                Err(_) => "[REDACTED URL]".into(),
            },
        )
    {
        diagnostic = redacted;
    }
    for (pattern, replacement) in CREDENTIALS.iter() {
        if let Cow::Owned(redacted) = pattern.replace_all(&diagnostic, *replacement) {
            diagnostic = redacted;
        }
    }
    diagnostic
}

#[cfg(test)]
#[path = "../tests/error_diagnostic_tests.rs"]
mod diagnostic_tests;

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
