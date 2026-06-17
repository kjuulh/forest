//! Developer Settings — organisation-owned OAuth applications.
//!
//! These pages let org admins register and manage "Sign in with Forest" OAuth
//! apps. All state lives in forest-server; this module is the UI + delegation
//! layer (mirrors the `integrations` module's shape). The client_secret is
//! shown exactly once, immediately after create / rotate.
//!
//! `result_large_err` is allowed module-wide, matching the rest of `routes/`:
//! handlers return `Result<Response, Response>` where `Response` is large.
#![allow(clippy::result_large_err)]

use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use forage_core::platform::{validate_slug, OAuthApp, PlatformError};
use forage_core::session::CachedOrg;
use minijinja::context;
use serde::Deserialize;

use super::{error_page, internal_error, orgs_context};
use crate::auth::Session;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/orgs/{org}/settings/developers", get(list_apps))
        .route("/orgs/{org}/settings/developers", post(create_app))
        .route("/orgs/{org}/settings/developers/new", get(new_app_page))
        .route("/orgs/{org}/settings/developers/{app_id}", get(app_detail))
        .route(
            "/orgs/{org}/settings/developers/{app_id}",
            post(update_app),
        )
        .route(
            "/orgs/{org}/settings/developers/{app_id}/rotate",
            post(rotate_secret),
        )
        .route(
            "/orgs/{org}/settings/developers/{app_id}/delete",
            post(delete_app),
        )
}

// ─── Shared guards (per the integrations.rs convention) ──────────────

fn require_org_membership<'a>(
    state: &AppState,
    orgs: &'a [CachedOrg],
    org: &str,
) -> Result<&'a CachedOrg, Response> {
    if !validate_slug(org) {
        return Err(error_page(
            state,
            axum::http::StatusCode::BAD_REQUEST,
            "Invalid request",
            "Invalid organisation name.",
        ));
    }
    orgs.iter().find(|o| o.name == org).ok_or_else(|| {
        error_page(
            state,
            axum::http::StatusCode::FORBIDDEN,
            "Access denied",
            "You are not a member of this organisation.",
        )
    })
}

fn require_admin(state: &AppState, org: &CachedOrg) -> Result<(), Response> {
    if org.role == "owner" || org.role == "admin" {
        Ok(())
    } else {
        Err(error_page(
            state,
            axum::http::StatusCode::FORBIDDEN,
            "Access denied",
            "You must be an admin to manage OAuth applications.",
        ))
    }
}

fn require_oauth_client(
    state: &AppState,
) -> Result<&std::sync::Arc<dyn forage_core::platform::ForestOAuthApps>, Response> {
    state.oauth_apps_client.as_ref().ok_or_else(|| {
        error_page(
            state,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "OAuth application management is not configured on this server.",
        )
    })
}

fn validate_csrf(session: &Session, form_csrf: &str) -> Result<(), Response> {
    if session.csrf_token == form_csrf {
        Ok(())
    } else {
        Err((axum::http::StatusCode::FORBIDDEN, "CSRF token mismatch").into_response())
    }
}

/// Map a forest-side platform error to a user-facing message for re-display on
/// the form (validation failures) vs a hard error page (everything else).
fn flash_for(err: &PlatformError) -> Option<String> {
    match err {
        PlatformError::InvalidArgument(m)
        | PlatformError::AlreadyExists(m)
        | PlatformError::PermissionDenied(m) => Some(m.clone()),
        _ => None,
    }
}

// ─── View-model helpers ──────────────────────────────────────────────

fn app_context(app: &OAuthApp) -> minijinja::Value {
    context! {
        app_id => &app.app_id,
        name => &app.name,
        description => &app.description,
        homepage_url => &app.homepage_url,
        client_id => &app.client_id,
        redirect_uris => &app.redirect_uris,
        scopes => &app.scopes,
    }
}

// ─── Forms / queries ─────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct PageQuery {
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct AppForm {
    #[serde(rename = "_csrf")]
    csrf: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    homepage_url: String,
    #[serde(default)]
    redirect_uris: String,
    #[serde(default)]
    scope_openid: Option<String>,
    #[serde(default)]
    scope_profile: Option<String>,
    #[serde(default)]
    scope_email: Option<String>,
}

impl AppForm {
    /// Split the redirect-URI textarea (one per line) into trimmed entries.
    fn redirect_uri_list(&self) -> Vec<String> {
        self.redirect_uris
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    fn scope_list(&self) -> Vec<String> {
        let mut scopes = Vec::new();
        if self.scope_openid.is_some() {
            scopes.push("openid".to_string());
        }
        if self.scope_profile.is_some() {
            scopes.push("profile".to_string());
        }
        if self.scope_email.is_some() {
            scopes.push("email".to_string());
        }
        scopes
    }
}

#[derive(Deserialize)]
struct CsrfForm {
    #[serde(rename = "_csrf")]
    csrf: String,
}

// ─── List ────────────────────────────────────────────────────────────

async fn list_apps(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    let client = require_oauth_client(&state)?;

    let apps = client
        .list_oauth_apps(&session.access_token, &cached_org.organisation_id)
        .await
        .map_err(|e| internal_error(&state, "list oauth apps", &e))?;

    let html = state
        .templates
        .render(
            "pages/developers.html.jinja",
            context! {
                title => format!("Developer settings - {org} - Forest"),
                description => "Manage OAuth applications",
                user => context! {
                    username => &session.user.username,
                    user_id => &session.user.user_id,
                },
                current_org => &org,
                orgs => orgs_context(&session.user.orgs),
                csrf_token => &session.csrf_token,
                active_tab => "settings",
                apps => apps.iter().map(app_context).collect::<Vec<_>>(),
                error => query.error,
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

// ─── New app form ────────────────────────────────────────────────────

async fn new_app_page(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    require_oauth_client(&state)?;

    let html = state
        .templates
        .render(
            "pages/developer_app_new.html.jinja",
            context! {
                title => format!("New OAuth app - {org} - Forest"),
                description => "Register a new OAuth application",
                user => context! {
                    username => &session.user.username,
                    user_id => &session.user.user_id,
                },
                current_org => &org,
                orgs => orgs_context(&session.user.orgs),
                csrf_token => &session.csrf_token,
                active_tab => "settings",
                error => query.error,
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;

    Ok(Html(html).into_response())
}

// ─── Create ──────────────────────────────────────────────────────────

async fn create_app(
    State(state): State<AppState>,
    session: Session,
    Path(org): Path<String>,
    Form(form): Form<AppForm>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    let client = require_oauth_client(&state)?;
    validate_csrf(&session, &form.csrf)?;

    let created = match client
        .create_oauth_app(
            &session.access_token,
            &cached_org.organisation_id,
            form.name.trim(),
            form.description.trim(),
            form.homepage_url.trim(),
            &form.redirect_uri_list(),
            &form.scope_list(),
        )
        .await
    {
        Ok(created) => created,
        Err(e) => {
            if let Some(msg) = flash_for(&e) {
                let q = urlencoding::encode(&msg);
                return Ok(Redirect::to(&format!(
                    "/orgs/{org}/settings/developers/new?error={q}"
                ))
                .into_response());
            }
            return Err(internal_error(&state, "create oauth app", &e));
        }
    };

    // Render the detail page directly so the freshly-minted secret can be
    // shown exactly once (it is never retrievable again).
    render_app_detail(&state, &session, &org, &created.app, Some(&created.client_secret))
}

// ─── Detail ──────────────────────────────────────────────────────────

async fn app_detail(
    State(state): State<AppState>,
    session: Session,
    Path((org, app_id)): Path<(String, String)>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    let client = require_oauth_client(&state)?;

    let app = client
        .get_oauth_app(&session.access_token, &cached_org.organisation_id, &app_id)
        .await
        .map_err(|e| match e {
            PlatformError::NotFound(_) => error_page(
                &state,
                axum::http::StatusCode::NOT_FOUND,
                "Not found",
                "That OAuth application does not exist.",
            ),
            other => internal_error(&state, "get oauth app", &other),
        })?;

    render_app_detail(&state, &session, &org, &app, None)
}

fn render_app_detail(
    state: &AppState,
    session: &Session,
    org: &str,
    app: &OAuthApp,
    client_secret: Option<&str>,
) -> Result<Response, Response> {
    let html = state
        .templates
        .render(
            "pages/developer_app.html.jinja",
            context! {
                title => format!("{} - Developer settings - Forest", app.name),
                description => "OAuth application",
                user => context! {
                    username => &session.user.username,
                    user_id => &session.user.user_id,
                },
                current_org => org,
                orgs => orgs_context(&session.user.orgs),
                csrf_token => &session.csrf_token,
                active_tab => "settings",
                app => app_context(app),
                client_secret => client_secret,
            },
        )
        .map_err(|e| internal_error(state, "template error", &e))?;
    Ok(Html(html).into_response())
}

// ─── Update ──────────────────────────────────────────────────────────

async fn update_app(
    State(state): State<AppState>,
    session: Session,
    Path((org, app_id)): Path<(String, String)>,
    Form(form): Form<AppForm>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    let client = require_oauth_client(&state)?;
    validate_csrf(&session, &form.csrf)?;

    match client
        .update_oauth_app(
            &session.access_token,
            &cached_org.organisation_id,
            &app_id,
            form.name.trim(),
            form.description.trim(),
            form.homepage_url.trim(),
            &form.redirect_uri_list(),
            &form.scope_list(),
        )
        .await
    {
        Ok(_) => Ok(
            Redirect::to(&format!("/orgs/{org}/settings/developers/{app_id}")).into_response(),
        ),
        Err(e) => {
            if let Some(msg) = flash_for(&e) {
                let q = urlencoding::encode(&msg);
                Ok(Redirect::to(&format!(
                    "/orgs/{org}/settings/developers/{app_id}?error={q}"
                ))
                .into_response())
            } else {
                Err(internal_error(&state, "update oauth app", &e))
            }
        }
    }
}

// ─── Rotate secret ───────────────────────────────────────────────────

async fn rotate_secret(
    State(state): State<AppState>,
    session: Session,
    Path((org, app_id)): Path<(String, String)>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    let client = require_oauth_client(&state)?;
    validate_csrf(&session, &form.csrf)?;

    let rotated = client
        .rotate_oauth_app_secret(&session.access_token, &cached_org.organisation_id, &app_id)
        .await
        .map_err(|e| internal_error(&state, "rotate oauth secret", &e))?;

    render_app_detail(
        &state,
        &session,
        &org,
        &rotated.app,
        Some(&rotated.client_secret),
    )
}

// ─── Delete ──────────────────────────────────────────────────────────

async fn delete_app(
    State(state): State<AppState>,
    session: Session,
    Path((org, app_id)): Path<(String, String)>,
    Form(form): Form<CsrfForm>,
) -> Result<Response, Response> {
    let cached_org = require_org_membership(&state, &session.user.orgs, &org)?;
    require_admin(&state, cached_org)?;
    let client = require_oauth_client(&state)?;
    validate_csrf(&session, &form.csrf)?;

    client
        .delete_oauth_app(&session.access_token, &cached_org.organisation_id, &app_id)
        .await
        .map_err(|e| internal_error(&state, "delete oauth app", &e))?;

    Ok(Redirect::to(&format!("/orgs/{org}/settings/developers")).into_response())
}
