use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use forage_core::platform::validate_slug;
use minijinja::context;
use serde::Deserialize;

use super::{
    error_page, internal_error, orgs_context, render_markdown, require_org_membership,
    warn_default,
};
use crate::auth::{MaybeSession, Session};
use crate::manifest_view::ManifestView;
use crate::pretty_json;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    // One surface for every artefact (specs/features/007). Components and
    // tools live on the same routes; the detail page renders shape-aware
    // sections (install / methods / upstream / releases / manifest).
    Router::new()
        .route("/components", get(components_search))
        .route("/components/{org}/{name}", get(component_detail))
        .route(
            "/components/{org}/{name}/{version}",
            get(component_version_detail),
        )
        .route("/orgs/{org}/components", get(org_components))
        // Project-level Components tab — lists every version published
        // under this project. Sibling to Releases (the deployment timeline).
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

/// Build the (description, metadata) pair the project + component
/// templates need for the header and About sidebar. Project description
/// wins; the component manifest description is the fallback. See spec
/// 009 §"Forage UI".
fn project_overview_ctx(
    project: Option<&forage_core::platform::Project>,
    component_description: Option<&str>,
) -> (String, Option<minijinja::Value>) {
    let description = project
        .map(|p| p.description.clone())
        .filter(|d| !d.is_empty())
        .or_else(|| component_description.map(|s| s.to_string()))
        .unwrap_or_default();

    let metadata_ctx = project.map(|p| {
        let m = &p.metadata;
        context! {
            git_url => &m.git_url,
            homepage => &m.homepage,
            docs_url => &m.docs_url,
            support_url => &m.support_url,
            domain => &m.domain,
            owner => &m.owner,
            has_any => !m.is_empty(),
        }
    });

    (description, metadata_ctx)
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
                active_tab => "components",
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

/// GET /components/{org}/{name} — component detail page.
///
/// Per specs/features/008, when a project with the same name exists in
/// this org we 303-redirect to the canonical project Overview. The legacy
/// component-detail page only renders for orphaned components (no
/// matching project) so external links continue to resolve.
async fn component_detail(
    State(state): State<AppState>,
    maybe_session: MaybeSession,
    Path((org, name)): Path<(String, String)>,
) -> Result<Response, Response> {
    // Project canonicalisation: redirect to the project Overview when a
    // same-named project exists. Only consult `list_projects` for sessions
    // that have an access token — anonymous traffic can't pass an org
    // membership check, so it skips the lookup and lands on the legacy
    // page directly (matches public-component-detail behaviour). 303 = soft
    // redirect; reversible. A `list_projects` failure is logged then skipped
    // — degrades to the legacy page rather than 5xx on a transient blip.
    if let Some(ref session) = maybe_session.session {
        match state
            .platform_client
            .list_projects(&session.access_token, &org)
            .await
        {
            Ok(projects) => {
                if projects.iter().any(|p| p == &name) {
                    return Ok(Redirect::to(&format!("/orgs/{org}/projects/{name}"))
                        .into_response());
                }
            }
            Err(e) => {
                tracing::warn!(
                    "component_detail: list_projects({org}) failed; falling through to legacy page: {e:#}"
                );
            }
        }
    }

    let registry = require_registry(&state)?;
    let token = resolve_token(&maybe_session, &state);

    let (detail_res, project_res) = tokio::join!(
        registry.get_component_detail(&token, &org, &name),
        state.platform_client.get_project(&token, &org, &name),
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
    // get_project is best-effort: a missing project (component without
    // a matching project row) still renders the component page; the
    // About sidebar block just stays hidden in that case.
    let project_info = match project_res {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("component_detail: get_project failed: {e:#}");
            None
        }
    };

    let readme_html = if detail.readme.is_empty() {
        String::new()
    } else {
        render_markdown(&detail.readme)
    };

    let manifest_html = if detail.manifest_json.is_empty() {
        String::new()
    } else {
        pretty_json::tokenize(&detail.manifest_json)
    };
    let manifest_view = ManifestView::parse(&detail.manifest_json);

    let (user, csrf_token, orgs, current_org) = maybe_user_context(&maybe_session);

    let (project_description, project_metadata_ctx) =
        project_overview_ctx(
            project_info.as_ref(),
            Some(detail.summary.description.as_str()).filter(|s| !s.is_empty()),
        );

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
                manifest_html => manifest_html,
                manifest => manifest_view,
                owners => &detail.owners,
                project_description => &project_description,
                project_metadata => project_metadata_ctx,
                active_detail_tab => "readme",
                user => user,
                csrf_token => csrf_token,
                orgs => orgs,
                current_org => current_org,
                active_tab => "components",
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

    let (detail_res, manifest_res, project_res) = tokio::join!(
        registry.get_component_detail(&token, &org, &name),
        registry.get_component_manifest(&token, &org, &name, &version),
        state.platform_client.get_project(&token, &org, &name),
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
    let project_info = match project_res {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("component_version_detail: get_project failed: {e:#}");
            None
        }
    };

    let manifest_json = warn_default("get_component_manifest", manifest_res);
    let manifest_html = if manifest_json.is_empty() {
        String::new()
    } else {
        pretty_json::tokenize(&manifest_json)
    };
    let manifest_view = ManifestView::parse(&manifest_json);

    let readme_html = if detail.readme.is_empty() {
        String::new()
    } else {
        render_markdown(&detail.readme)
    };

    let (user, csrf_token, orgs, current_org) = maybe_user_context(&maybe_session);

    let (project_description, project_metadata_ctx) =
        project_overview_ctx(
            project_info.as_ref(),
            Some(detail.summary.description.as_str()).filter(|s| !s.is_empty()),
        );

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
                manifest_html => manifest_html,
                manifest => manifest_view,
                owners => &detail.owners,
                selected_version => &version,
                project_description => &project_description,
                project_metadata => project_metadata_ctx,
                active_detail_tab => "readme",
                user => user,
                csrf_token => csrf_token,
                orgs => orgs,
                current_org => current_org,
                active_tab => "components",
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


/// `GET /orgs/{org}/projects/{project}/components` — project's Components tab.
///
/// Lists every published version. Distinct from the Overview's sidebar
/// summary (top 3 + "see all") and from the global `/orgs/{org}/components`
/// (across-projects search).
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
                title => format!("Components - {project} - {org} - Forage"),
                description => format!("Published components for {project}"),
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
