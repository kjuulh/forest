mod auth;
mod events;
mod integrations;
mod pages;
mod platform;
mod registry;

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
        .merge(platform::router())
        .merge(events::router())
        .merge(integrations::router())
        .merge(registry::router())
}

/// Render an error page with the given status code, heading, and message.
fn error_page(state: &AppState, status: StatusCode, heading: &str, message: &str) -> Response {
    error_page_detail(state, status, heading, message, None)
}

/// Render an error page with optional error detail (shown in a collapsible section).
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
            title => format!("{} - Forage", heading),
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

/// Log an error and render a 500 page with the error detail.
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

/// Log a warning for a failed call and return the default value.
/// Use for supplementary data where graceful degradation is acceptable.
fn warn_default<T: Default>(context: &str, result: Result<T, impl std::fmt::Display>) -> T {
    match result {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("{context}: {e:#}");
            T::default()
        }
    }
}

/// Render markdown → sanitised HTML. Shared by the registry and platform
/// routes so the project Overview and the legacy component-detail page
/// produce identical output for the same source markdown.
pub(super) fn render_markdown(md: &str) -> String {
    let parser = pulldown_cmark::Parser::new(md);
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, parser);
    ammonia::clean(&html)
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
