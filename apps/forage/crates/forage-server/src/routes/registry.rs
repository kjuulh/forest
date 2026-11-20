use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use forage_core::platform::validate_slug;
use serde::Deserialize;

use super::{
    default_unless_missing, error_page, internal_error, render_markdown, require_org_membership,
    warn_default,
};
use crate::auth::{MaybeSession, Session};
use crate::manifest_view::ManifestView;
use crate::pretty_json;
use crate::state::AppState;
use crate::views::catalogue::{
    ComponentDetailPage, ComponentsPage, OrgComponentsPage, registry_chrome,
};

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

fn require_registry(
    state: &AppState,
) -> Result<&dyn forage_core::registry::ForestRegistry, Response> {
    state.registry_client.as_deref().ok_or_else(|| {
        error_page(
            state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Registry unavailable",
            "Component registry is not configured.",
        )
    })
}

fn project_overview(
    project: Option<&forage_core::platform::Project>,
    component_description: Option<&str>,
) -> (String, forage_ui::about::About) {
    let description = project
        .map(|p| p.description.clone())
        .filter(|d| !d.is_empty())
        .or_else(|| component_description.map(|s| s.to_string()))
        .unwrap_or_default();
    let about = project
        .map(|p| crate::views::projects::about(&p.metadata))
        .unwrap_or_default();
    (description, about)
}

fn dedup_components(
    components: Vec<forage_core::registry::ComponentSummary>,
) -> Vec<forage_core::registry::ComponentSummary> {
    let mut seen = std::collections::HashSet::new();
    components
        .into_iter()
        .filter(|c| seen.insert((c.organisation.clone(), c.name.clone())))
        .collect()
}

fn total_pages(total_count: i32, page_size: i32) -> u32 {
    ((total_count as f64) / (page_size as f64)).ceil().max(0.0) as u32
}

fn manifest_parts(json: &str) -> (Option<ManifestView>, Option<sedge_rt::Trusted>) {
    if json.is_empty() {
        return (ManifestView::parse(json), None);
    }
    (
        ManifestView::parse(json),
        Some(sedge_rt::Trusted(pretty_json::tokenize(json))),
    )
}

fn readme(markdown: &str) -> Option<sedge_rt::Trusted> {
    Some(markdown)
        .filter(|md| !md.is_empty())
        .map(render_markdown)
        .filter(|html| !html.is_empty())
        .map(sedge_rt::Trusted)
}

async fn components_search(
    State(state): State<AppState>,
    maybe_session: MaybeSession,
    Query(params): Query<SearchParams>,
) -> Result<Response, Response> {
    let registry = require_registry(&state)?;

    let query = params.q.unwrap_or_default();
    let filter_org = params.org.unwrap_or_default();
    let page = params.page.unwrap_or(1).max(1);
    let page_size = 20;

    let results = registry
        .search_public_components(
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
        .map_err(|e| internal_error(&state, "search_public_components", &e))?;

    let components = dedup_components(results.components);
    let total_pages = total_pages(results.total_count, page_size);
    let session = maybe_session.session.as_ref();
    let page = ComponentsPage {
        chrome: registry_chrome(
            &state,
            session,
            "Components - Forest".into(),
            "Discover and share reusable forest components.".into(),
        ),
        query,
        filter_org,
        orgs: session
            .map(|s| s.user.orgs.iter().map(|o| o.name.clone()).collect())
            .unwrap_or_default(),
        components,
        page: page as u32,
        total_pages,
    };
    let html = crate::views::pages::components::render(&page);

    Ok(Html(html).into_response())
}

async fn component_detail(
    State(state): State<AppState>,
    maybe_session: MaybeSession,
    Path((org, name)): Path<(String, String)>,
) -> Result<Response, Response> {
    if let Some(ref session) = maybe_session.session {
        match state
            .platform_client
            .list_projects(&session.access_token, &org)
            .await
        {
            Ok(projects) => {
                if projects.iter().any(|p| p == &name) {
                    return Ok(
                        Redirect::to(&format!("/orgs/{org}/projects/{name}")).into_response()
                    );
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

    let (detail_res, project_info) = match maybe_session.session {
        Some(ref session) => {
            let (d, p) = tokio::join!(
                registry.get_component_detail(&session.access_token, &org, &name),
                state
                    .platform_client
                    .get_project(&session.access_token, &org, &name),
            );
            let project_info = match p {
                Ok(info) => info,
                Err(e) => {
                    tracing::warn!("component_detail: get_project failed: {e:#}");
                    None
                }
            };
            (d, project_info)
        }
        None => (
            registry.get_public_component_detail(&org, &name).await,
            None,
        ),
    };

    let detail = detail_res.map_err(|e| match e {
        forage_core::platform::PlatformError::NotFound(_) => error_page(
            &state,
            StatusCode::NOT_FOUND,
            "Component not found",
            &format!("The component {org}/{name} does not exist or you don't have access."),
        ),
        other => internal_error(&state, "get_component_detail", &other),
    })?;

    let (manifest, manifest_html) = manifest_parts(&detail.manifest_json);
    let (description, about) = project_overview(
        project_info.as_ref(),
        Some(detail.summary.description.as_str()).filter(|s| !s.is_empty()),
    );

    let page = ComponentDetailPage {
        chrome: registry_chrome(
            &state,
            maybe_session.session.as_ref(),
            format!("{org}/{name} - Components - Forest"),
            detail.summary.description.clone(),
        ),
        readme: readme(&detail.readme),
        summary: detail.summary,
        versions: detail.versions,
        selected_version: None,
        description,
        manifest,
        manifest_html,
        owners: detail.owners,
        about,
        highlight_js: state.templates.asset_href("js/readme-highlight.js"),
    };
    let html = crate::views::pages::component_detail::render(&page);

    Ok(Html(html).into_response())
}

async fn component_version_detail(
    State(state): State<AppState>,
    maybe_session: MaybeSession,
    Path((org, name, version)): Path<(String, String, String)>,
) -> Result<Response, Response> {
    let registry = require_registry(&state)?;

    let (detail_res, manifest_res, project_info) = match maybe_session.session {
        Some(ref session) => {
            let (d, m, p) = tokio::join!(
                registry.get_component_detail(&session.access_token, &org, &name),
                registry.get_component_manifest(&session.access_token, &org, &name, &version),
                state
                    .platform_client
                    .get_project(&session.access_token, &org, &name),
            );
            let project_info = match p {
                Ok(info) => info,
                Err(e) => {
                    tracing::warn!("component_version_detail: get_project failed: {e:#}");
                    None
                }
            };
            (d, m, project_info)
        }
        None => {
            let (d, m) = tokio::join!(
                registry.get_public_component_detail(&org, &name),
                registry.get_public_component_manifest(&org, &name, &version),
            );
            (d, m, None)
        }
    };

    let detail = detail_res.map_err(|e| match e {
        forage_core::platform::PlatformError::NotFound(_) => error_page(
            &state,
            StatusCode::NOT_FOUND,
            "Component not found",
            &format!("The component {org}/{name} does not exist or you don't have access."),
        ),
        other => internal_error(&state, "get_component_detail", &other),
    })?;

    let manifest_json = default_unless_missing("get_public_component_manifest", manifest_res);
    let (manifest, manifest_html) = manifest_parts(&manifest_json);
    let (description, about) = project_overview(
        project_info.as_ref(),
        Some(detail.summary.description.as_str()).filter(|s| !s.is_empty()),
    );

    let page = ComponentDetailPage {
        chrome: registry_chrome(
            &state,
            maybe_session.session.as_ref(),
            format!("{org}/{name}@{version} - Components - Forest"),
            detail.summary.description.clone(),
        ),
        readme: readme(&detail.readme),
        summary: detail.summary,
        versions: detail.versions,
        selected_version: Some(version),
        description,
        manifest,
        manifest_html,
        owners: detail.owners,
        about,
        highlight_js: state.templates.asset_href("js/readme-highlight.js"),
    };
    let html = crate::views::pages::component_detail::render(&page);

    Ok(Html(html).into_response())
}

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
    let total_pages = total_pages(results.total_count, page_size);

    let page = OrgComponentsPage {
        chrome: crate::views::chrome::signed_in(
            &state,
            &session,
            crate::views::chrome::Page {
                title: format!("{org} - Components - Forest"),
                description: format!("Components in {org}"),
                org: Some(&org),
                project: None,
                projects: Vec::new(),
                tab: forage_ui::chrome::Tab::Components,
            },
        ),
        org,
        query,
        components,
        page: page as u32,
        total_pages,
    };
    let html = crate::views::pages::org_components::render(&page);

    Ok(Html(html).into_response())
}

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

    let page = crate::views::projects::ProjectComponentsPage {
        chrome: crate::views::chrome::signed_in(
            &state,
            &session,
            crate::views::chrome::Page {
                title: format!("Components - {project} - {org} - Forest"),
                description: format!("Published components for {project}"),
                org: Some(&org),
                project: Some(&project),
                projects,
                tab: forage_ui::chrome::Tab::ProjectComponents,
            },
        ),
        org: org.clone(),
        project: project.clone(),
        versions,
    };
    let html = crate::views::pages::project_components::render(&page);

    Ok(Html(html).into_response())
}
