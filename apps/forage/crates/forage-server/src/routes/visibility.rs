use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum_extra::extract::Form;
use forage_core::platform::{PlatformError, validate_slug, validate_visibility};
use forage_core::session::CachedOrg;
use serde::Deserialize;

use super::{error_page, internal_error, report_error, require_org_membership, warn_default};
use crate::auth::{self, Session};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/orgs/{org}/settings/visibility",
            get(org_page).post(org_submit),
        )
        .route(
            "/orgs/{org}/projects/{project}/settings/visibility",
            get(project_page).post(project_submit),
        )
}

#[derive(Deserialize)]
struct VisibilityForm {
    visibility: String,
    _csrf: String,
}

fn require_admin(state: &AppState, org: &CachedOrg) -> Result<(), Response> {
    if org.role == "owner" || org.role == "admin" {
        Ok(())
    } else {
        Err(error_page(
            state,
            StatusCode::FORBIDDEN,
            "Access denied",
            "Only an organisation admin can change visibility.",
        ))
    }
}

fn require_project_slug(state: &AppState, project: &str) -> Result<(), Response> {
    if validate_slug(project) {
        Ok(())
    } else {
        Err(error_page(
            state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid project name.",
        ))
    }
}

fn validate_form(
    state: &AppState,
    session: &Session,
    form: &VisibilityForm,
) -> Result<(), Response> {
    if !auth::validate_csrf(session, &form._csrf) {
        return Err(error_page(
            state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }
    if !validate_visibility(&form.visibility) {
        return Err(error_page(
            state,
            StatusCode::BAD_REQUEST,
            "Invalid request",
            "Visibility must be public or private.",
        ));
    }
    Ok(())
}

fn refusal_status(error: &PlatformError) -> Option<(StatusCode, String)> {
    let status = match error {
        PlatformError::PermissionDenied(_) => StatusCode::FORBIDDEN,
        PlatformError::InvalidArgument(_) => StatusCode::BAD_REQUEST,
        PlatformError::NotFound(_) => StatusCode::NOT_FOUND,
        _ => return None,
    };
    Some((status, report_error("change visibility", error)))
}

fn read_error(state: &AppState, error: PlatformError) -> Response {
    match error {
        PlatformError::PermissionDenied(_) => error_page(
            state,
            StatusCode::FORBIDDEN,
            "Access denied",
            "You don't have access to this organisation.",
        ),
        PlatformError::NotFound(_) => error_page(
            state,
            StatusCode::NOT_FOUND,
            "Not found",
            "This organisation or project does not exist.",
        ),
        e => internal_error(state, "get_visibility", &e),
    }
}

async fn org_page(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
) -> Result<Response, Response> {
    render_org(state, session, org, None).await
}

async fn render_org(
    state: AppState,
    session: Session,
    org: String,
    flash_error: Option<(StatusCode, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;

    let visibility = state
        .platform_client
        .get_visibility(&session.access_token, &org, None)
        .await
        .map_err(|e| read_error(&state, e))?;

    let (status, flash_error) = match flash_error {
        Some((status, msg)) => (status, Some(msg)),
        None => (StatusCode::OK, None),
    };

    let page = crate::views::org_pages::OrgVisibilityPage {
        chrome: crate::views::org_settings::chrome(
            &state,
            &session,
            &org,
            format!("Visibility - {org} - Forest"),
            format!("Who can read {org}'s components"),
        ),
        org: org.clone(),
        csrf_token: session.csrf_token.clone(),
        org_public: visibility.organisation_is_public(),
        can_change: visibility.can_change,
        flash_error,
    };
    let html = crate::views::pages::org_visibility::render(&page);

    Ok((status, Html(html)).into_response())
}

async fn org_submit(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Form(form): Form<VisibilityForm>,
) -> Result<Response, Response> {
    let current_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, current_org)?;
    validate_form(&state, &session, &form)?;

    match state
        .platform_client
        .set_organisation_visibility(&session.access_token, &org, &form.visibility)
        .await
    {
        Ok(()) => Ok(Redirect::to(&format!("/orgs/{org}/settings/visibility")).into_response()),
        Err(e) => match refusal_status(&e) {
            Some(refusal) => render_org(state, session, org, Some(refusal)).await,
            None => Err(internal_error(&state, "set_organisation_visibility", &e)),
        },
    }
}

async fn project_page(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
) -> Result<Response, Response> {
    render_project(state, session, org, project, None).await
}

async fn render_project(
    state: AppState,
    session: Session,
    org: String,
    project: String,
    flash_error: Option<(StatusCode, String)>,
) -> Result<Response, Response> {
    let orgs = &session.user.orgs;
    require_org_membership(&state, orgs, &org)?;
    require_project_slug(&state, &project)?;

    let visibility = state
        .platform_client
        .get_visibility(&session.access_token, &org, Some(&project))
        .await
        .map_err(|e| read_error(&state, e))?;

    let (status, flash_error) = match flash_error {
        Some((status, msg)) => (status, Some(msg)),
        None => (StatusCode::OK, None),
    };

    let projects = warn_default(
        "project visibility: list projects",
        state
            .platform_client
            .list_projects(&session.access_token, &org)
            .await,
    );
    let page = crate::views::org_pages::ProjectVisibilityPage {
        chrome: crate::views::chrome::signed_in(
            &state,
            &session,
            crate::views::chrome::Page {
                title: format!("Visibility - {project} - {org} - Forest"),
                description: format!("Who can read {org}/{project}"),
                org: Some(&org),
                project: Some(&project),
                projects,
                tab: forage_ui::chrome::Tab::ProjectSettings,
            },
        ),
        org: org.clone(),
        project: project.clone(),
        csrf_token: session.csrf_token.clone(),
        org_public: visibility.organisation_is_public(),
        project_public: visibility.project_is_public(),
        component_public: visibility.component_is_public(),
        can_change: visibility.can_change,
        flash_error,
    };
    let html = crate::views::pages::project_visibility::render(&page);

    Ok((status, Html(html)).into_response())
}

async fn project_submit(
    State(state): State<AppState>,
    session: Session,
    Path((org, project)): Path<(String, String)>,
    Form(form): Form<VisibilityForm>,
) -> Result<Response, Response> {
    let current_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, current_org)?;
    require_project_slug(&state, &project)?;
    validate_form(&state, &session, &form)?;

    match state
        .platform_client
        .set_project_visibility(&session.access_token, &org, &project, &form.visibility)
        .await
    {
        Ok(()) => Ok(Redirect::to(&format!(
            "/orgs/{org}/projects/{project}/settings/visibility"
        ))
        .into_response()),
        Err(e) => match refusal_status(&e) {
            Some(refusal) => render_project(state, session, org, project, Some(refusal)).await,
            None => Err(internal_error(&state, "set_project_visibility", &e)),
        },
    }
}
