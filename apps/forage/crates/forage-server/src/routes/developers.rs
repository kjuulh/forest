#![allow(clippy::result_large_err)]

use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use forage_core::platform::{OAuthApp, PlatformError, validate_slug};
use forage_core::session::CachedOrg;
use minijinja::context;
use serde::Deserialize;

use super::{error_page, internal_error, orgs_context, report_error};
use crate::auth::Session;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/orgs/{org}/settings/developers", get(list_apps))
        .route("/orgs/{org}/settings/developers", post(create_app))
        .route("/orgs/{org}/settings/developers/new", get(new_app_page))
        .route("/orgs/{org}/settings/developers/{app_id}", get(app_detail))
        .route("/orgs/{org}/settings/developers/{app_id}", post(update_app))
        .route(
            "/orgs/{org}/settings/developers/{app_id}/rotate",
            post(rotate_secret),
        )
        .route(
            "/orgs/{org}/settings/developers/{app_id}/delete",
            post(delete_app),
        )
}

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

fn flash_for(err: &PlatformError) -> Option<String> {
    match err {
        PlatformError::InvalidArgument(_)
        | PlatformError::AlreadyExists(_)
        | PlatformError::PermissionDenied(_) => {
            Some(report_error("OAuth application settings", err))
        }
        _ => None,
    }
}

fn app_context(app: &OAuthApp) -> minijinja::Value {
    context! {
        app_id => &app.app_id,
        name => &app.name,
        description => &app.description,
        homepage_url => &app.homepage_url,
        client_id => &app.client_id,
        redirect_uris => &app.redirect_uris,
        scopes => &app.scopes,
        grant_types => &app.grant_types,
    }
}

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
    #[serde(default)]
    scope_components_publish: Option<String>,
    #[serde(default)]
    scope_tokens_introspect: Option<String>,
    #[serde(default)]
    scope_datagateway_apply: Option<String>,
    #[serde(default)]
    scope_datagateway_read: Option<String>,
    #[serde(default)]
    grant_authorization_code: Option<String>,
    #[serde(default)]
    grant_client_credentials: Option<String>,
    #[serde(default)]
    grant_device_code: Option<String>,
}

const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

impl AppForm {
    fn grant_type_list(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.grant_authorization_code.is_some() {
            out.push("authorization_code".to_string());
        }
        if self.grant_client_credentials.is_some() {
            out.push("client_credentials".to_string());
        }
        if self.grant_device_code.is_some() {
            out.push(DEVICE_CODE_GRANT.to_string());
        }
        out
    }

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
        if self.scope_components_publish.is_some() {
            scopes.push("components:publish".to_string());
        }
        if self.scope_tokens_introspect.is_some() {
            scopes.push("tokens:introspect".to_string());
        }
        if self.scope_datagateway_apply.is_some() {
            scopes.push("datagateway:apply".to_string());
        }
        if self.scope_datagateway_read.is_some() {
            scopes.push("datagateway:read".to_string());
        }
        scopes
    }
}

#[derive(Deserialize)]
struct CsrfForm {
    #[serde(rename = "_csrf")]
    csrf: String,
}

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
            &form.grant_type_list(),
        )
        .await
    {
        Ok(created) => created,
        Err(e) => {
            if let Some(msg) = flash_for(&e) {
                let q = urlencoding::encode(&msg);
                return Ok(
                    Redirect::to(&format!("/orgs/{org}/settings/developers/new?error={q}"))
                        .into_response(),
                );
            }
            return Err(internal_error(&state, "create oauth app", &e));
        }
    };

    render_app_detail(
        &state,
        &session,
        &org,
        &created.app,
        Some(&created.client_secret),
    )
}

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
            &form.grant_type_list(),
        )
        .await
    {
        Ok(_) => {
            Ok(Redirect::to(&format!("/orgs/{org}/settings/developers/{app_id}")).into_response())
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn form(auth_code: bool, client_creds: bool) -> AppForm {
        AppForm {
            csrf: String::new(),
            name: "app".into(),
            description: String::new(),
            homepage_url: String::new(),
            redirect_uris: String::new(),
            scope_openid: None,
            scope_profile: None,
            scope_email: None,
            scope_components_publish: None,
            scope_tokens_introspect: None,
            scope_datagateway_apply: None,
            scope_datagateway_read: None,
            grant_authorization_code: auth_code.then(|| "on".to_string()),
            grant_client_credentials: client_creds.then(|| "on".to_string()),
            grant_device_code: None,
        }
    }

    #[test]
    fn the_device_box_keeps_the_device_grant() {
        let mut form = form(true, false);
        form.grant_device_code = Some("on".to_string());
        assert_eq!(
            form.grant_type_list(),
            vec![
                "authorization_code".to_string(),
                DEVICE_CODE_GRANT.to_string()
            ]
        );
    }

    #[test]
    fn both_boxes_ticked_yields_both_grants() {
        assert_eq!(
            form(true, true).grant_type_list(),
            vec![
                "authorization_code".to_string(),
                "client_credentials".to_string()
            ]
        );
    }

    #[test]
    fn each_box_yields_only_its_own_grant() {
        assert_eq!(
            form(true, false).grant_type_list(),
            vec!["authorization_code".to_string()]
        );
        assert_eq!(
            form(false, true).grant_type_list(),
            vec!["client_credentials".to_string()]
        );
    }

    #[test]
    fn no_boxes_defers_the_default_to_forest() {
        assert!(form(false, false).grant_type_list().is_empty());
    }

    #[test]
    fn the_component_publish_box_requests_that_scope() {
        let mut form = form(false, true);
        form.scope_components_publish = Some("on".to_string());
        assert_eq!(form.scope_list(), vec!["components:publish".to_string()]);
    }
}
