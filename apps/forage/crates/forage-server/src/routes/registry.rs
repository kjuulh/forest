use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use forage_core::platform::validate_slug;
use minijinja::context;
use serde::Deserialize;

use super::{error_page, internal_error, orgs_context, require_org_membership, warn_default};
use crate::auth::{MaybeSession, Session};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/components", get(components_search))
        .route("/components/{org}/{name}", get(component_detail))
        .route(
            "/components/{org}/{name}/{version}",
            get(component_version_detail),
        )
        .route("/orgs/{org}/components", get(org_components))
        .route(
            "/orgs/{org}/projects/{project}/components",
            get(project_components),
        )
}

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
    org: Option<String>,
    page: Option<i32>,
}

fn render_markdown(md: &str) -> String {
    let parser = pulldown_cmark::Parser::new(md);
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, parser);
    ammonia::clean(&html)
}

/// Resolve an access token: prefer user session, fall back to service account key, or empty (public).
fn resolve_token(maybe_session: &MaybeSession, state: &AppState) -> String {
    if let Some(ref session) = maybe_session.session {
        return session.access_token.clone();
    }
    state
        .service_account_key
        .clone()
        .unwrap_or_default()
}

fn require_registry(state: &AppState) -> Result<&dyn forage_core::registry::ForestRegistry, Response> {
    state
        .registry_client
        .as_deref()
        .ok_or_else(|| {
            error_page(
                state,
                StatusCode::SERVICE_UNAVAILABLE,
                "Registry unavailable",
                "Component registry is not configured.",
            )
        })
}

/// Deduplicate component summaries by (organisation, name), keeping the first occurrence
/// (which is the latest version since results are sorted by updated_at DESC).
fn dedup_components(
    components: Vec<forage_core::registry::ComponentSummary>,
) -> Vec<forage_core::registry::ComponentSummary> {
    let mut seen = std::collections::HashSet::new();
    components
        .into_iter()
        .filter(|c| seen.insert((c.organisation.clone(), c.name.clone())))
        .collect()
}

/// Extract optional user context from MaybeSession for template rendering.
fn maybe_user_context(
    maybe_session: &MaybeSession,
) -> (
    Option<minijinja::Value>,
    Option<String>,
    Vec<minijinja::Value>,
    Option<String>,
) {
    match maybe_session.session {
        Some(ref s) => (
            Some(context! { username => &s.user.username }),
            Some(s.csrf_token.clone()),
            orgs_context(&s.user.orgs),
            s.user.orgs.first().map(|o| o.name.clone()),
        ),
        None => (None, None, vec![], None),
    }
}

/// GET /components — public search/browse.
async fn components_search(
    State(state): State<AppState>,
    maybe_session: MaybeSession,
    Query(params): Query<SearchParams>,
) -> Result<Response, Response> {
    let registry = require_registry(&state)?;
    let token = resolve_token(&maybe_session, &state);

    let query = params.q.unwrap_or_default();
    let filter_org = params.org.unwrap_or_default();
    let page = params.page.unwrap_or(1).max(1);
    let page_size = 20;

    let results = registry
        .search_components(
            &token,
            &query,
            if filter_org.is_empty() {
                None
            } else {
                Some(&filter_org)
            },
            page,
            page_size,
        )
        .await
        .map_err(|e| internal_error(&state, "search_components", &e))?;

    let components = dedup_components(results.components);
    let total_pages = ((results.total_count as f64) / (page_size as f64)).ceil() as i32;
    let (user, csrf_token, orgs, current_org) = maybe_user_context(&maybe_session);

    let html = state
        .templates
        .render(
            "pages/components.html.jinja",
            context! {
                title => "Components - Forage",
                description => "Discover and share reusable forest components.",
                components => components,
                query => &query,
                filter_org => &filter_org,
                page => page,
                total_pages => total_pages,
                user => user,
                csrf_token => csrf_token,
                orgs => orgs,
                current_org => current_org,
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

/// GET /components/{org}/{name} — component detail page.
async fn component_detail(
    State(state): State<AppState>,
    maybe_session: MaybeSession,
    Path((org, name)): Path<(String, String)>,
) -> Result<Response, Response> {
    let registry = require_registry(&state)?;
    let token = resolve_token(&maybe_session, &state);

    let detail = registry
        .get_component_detail(&token, &org, &name)
        .await
        .map_err(|e| match e {
            forage_core::platform::PlatformError::NotFound(_) => error_page(
                &state,
                StatusCode::NOT_FOUND,
                "Component not found",
                &format!("The component {org}/{name} does not exist."),
            ),
            other => internal_error(&state, "get_component_detail", &other),
        })?;

    let readme_html = if detail.readme.is_empty() {
        String::new()
    } else {
        render_markdown(&detail.readme)
    };

    let manifest_pretty = if detail.manifest_json.is_empty() {
        String::new()
    } else {
        serde_json::from_str::<serde_json::Value>(&detail.manifest_json)
            .and_then(|v| serde_json::to_string_pretty(&v))
            .unwrap_or(detail.manifest_json.clone())
    };

    let (user, csrf_token, orgs, current_org) = maybe_user_context(&maybe_session);

    let html = state
        .templates
        .render(
            "pages/component_detail.html.jinja",
            context! {
                title => format!("{org}/{name} - Components - Forage"),
                description => &detail.summary.description,
                summary => &detail.summary,
                versions => &detail.versions,
                readme_html => readme_html,
                manifest_json => manifest_pretty,
                owners => &detail.owners,
                active_detail_tab => "readme",
                user => user,
                csrf_token => csrf_token,
                orgs => orgs,
                current_org => current_org,
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

/// GET /components/{org}/{name}/{version} — version-specific detail.
async fn component_version_detail(
    State(state): State<AppState>,
    maybe_session: MaybeSession,
    Path((org, name, version)): Path<(String, String, String)>,
) -> Result<Response, Response> {
    let registry = require_registry(&state)?;
    let token = resolve_token(&maybe_session, &state);

    let (detail_res, manifest_res) = tokio::join!(
        registry.get_component_detail(&token, &org, &name),
        registry.get_component_manifest(&token, &org, &name, &version),
    );

    let detail = detail_res.map_err(|e| match e {
        forage_core::platform::PlatformError::NotFound(_) => error_page(
            &state,
            StatusCode::NOT_FOUND,
            "Component not found",
            &format!("The component {org}/{name} does not exist."),
        ),
        other => internal_error(&state, "get_component_detail", &other),
    })?;

    let manifest_json = warn_default("get_component_manifest", manifest_res);
    let manifest_pretty = if manifest_json.is_empty() {
        String::new()
    } else {
        serde_json::from_str::<serde_json::Value>(&manifest_json)
            .and_then(|v| serde_json::to_string_pretty(&v))
            .unwrap_or(manifest_json)
    };

    let readme_html = if detail.readme.is_empty() {
        String::new()
    } else {
        render_markdown(&detail.readme)
    };

    let (user, csrf_token, orgs, current_org) = maybe_user_context(&maybe_session);

    let html = state
        .templates
        .render(
            "pages/component_detail.html.jinja",
            context! {
                title => format!("{org}/{name}@{version} - Components - Forage"),
                description => &detail.summary.description,
                summary => &detail.summary,
                versions => &detail.versions,
                readme_html => readme_html,
                manifest_json => manifest_pretty,
                owners => &detail.owners,
                selected_version => &version,
                active_detail_tab => "readme",
                user => user,
                csrf_token => csrf_token,
                orgs => orgs,
                current_org => current_org,
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

/// GET /orgs/{org}/components — org-scoped component list.
async fn org_components(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Query(params): Query<SearchParams>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;
    let registry = require_registry(&state)?;

    let query = params.q.unwrap_or_default();
    let page = params.page.unwrap_or(1).max(1);
    let page_size = 20;

    let results = registry
        .search_components(&session.access_token, &query, Some(&org), page, page_size)
        .await
        .map_err(|e| internal_error(&state, "search_components", &e))?;

    let components = dedup_components(results.components);
    let total_pages = ((results.total_count as f64) / (page_size as f64)).ceil() as i32;

    let html = state
        .templates
        .render(
            "pages/org_components.html.jinja",
            context! {
                title => format!("{org} - Components - Forage"),
                description => format!("Components in {org}"),
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => &org,
                orgs => orgs_context(orgs),
                org_name => &org,
                components => components,
                query => &query,
                page => page,
                total_pages => total_pages,
                active_tab => "components",
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

/// GET /orgs/{org}/projects/{project}/components — project-level component versions.
async fn project_components(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;

    if !validate_slug(&project) {
        return Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid project name.",
        ));
    }

    let registry = require_registry(&state)?;
    let versions = registry
        .list_component_versions(&session.access_token, &org, &project)
        .await
        .map_err(|e| internal_error(&state, "list_component_versions", &e))?;

    let projects = state
        .platform_client
        .list_projects(&session.access_token, &org)
        .await;
    let projects = warn_default("list_projects", projects);

    let html = state
        .templates
        .render(
            "pages/project_components.html.jinja",
            context! {
                title => format!("{project} - Components - {org} - Forage"),
                description => format!("Component versions for {project}"),
                user => context! { username => session.user.username },
                csrf_token => &session.csrf_token,
                current_org => &org,
                orgs => orgs_context(orgs),
                org_name => &org,
                project_name => &project,
                projects => projects,
                versions => versions,
                active_tab => "project_components",
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}
