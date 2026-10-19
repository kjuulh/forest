#![allow(clippy::result_large_err)]

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use minijinja::context;
use serde::Deserialize;

use forage_core::platform::PendingOAuthDeviceAuthorization;

use super::{error_page, internal_error};
use crate::auth::{self, Session};
use crate::routes::oauth::scope_description;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/device", get(device_page))
        .route("/device", post(device_submit))
}

#[derive(Deserialize)]
struct DeviceQuery {
    #[serde(default)]
    user_code: Option<String>,
}

async fn device_page(
    State(state): State<AppState>,
    session: Session,
    Query(params): Query<DeviceQuery>,
) -> Result<Response, Response> {
    if state.service_account_key.is_none() {
        return render_device(&state, &session, "", None, Some("unconfigured"));
    }
    let user_code_prefill = params.user_code.unwrap_or_default();
    let app = app_grant(&state, &user_code_prefill).await?;
    render_device_for(
        &state,
        &session,
        &user_code_prefill,
        app.as_ref(),
        None,
        None,
    )
}

async fn app_grant(
    state: &AppState,
    user_code: &str,
) -> Result<Option<PendingOAuthDeviceAuthorization>, Response> {
    let (Some(client), false) = (
        state.oauth_apps_client.as_ref(),
        user_code.trim().is_empty(),
    ) else {
        return Ok(None);
    };
    client
        .lookup_oauth_device_authorization(user_code.trim())
        .await
        .map_err(|e| internal_error(state, "lookup device authorization", &e))
}

#[derive(Deserialize)]
struct DeviceForm {
    user_code: String,
    action: String,
    _csrf: String,
    #[serde(default)]
    client_id: String,
}

async fn device_submit(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<DeviceForm>,
) -> Result<Response, Response> {
    if state.service_account_key.is_none() {
        return render_device(&state, &session, "", None, Some("unconfigured"));
    }
    if !auth::validate_csrf(&session, &form._csrf) {
        return Err(error_page(
            &state,
            StatusCode::FORBIDDEN,
            "Invalid request",
            "CSRF validation failed. Please try again.",
        ));
    }

    let user_code = form.user_code.trim();
    if user_code.is_empty() {
        return render_device(
            &state,
            &session,
            "",
            Some("Enter the code shown in your terminal."),
            None,
        );
    }

    let approving_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let approving_user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let user_id = &session.user.user_id;

    if let Some(app) = app_grant(&state, user_code).await? {
        return decide_app_grant(
            &state,
            &session,
            user_code,
            &app,
            &form,
            &approving_ip,
            &approving_user_agent,
        )
        .await;
    }

    match form.action.as_str() {
        "approve" => match state
            .forest_client
            .approve_device_login(user_code, user_id, &approving_ip, &approving_user_agent)
            .await
        {
            Ok(()) => render_device(&state, &session, user_code, None, Some("approved")),
            Err(e) => {
                tracing::warn!(error = %e, "device login approval failed");
                render_device(
                    &state,
                    &session,
                    user_code,
                    Some(
                        "That code wasn't recognised, or has already expired. Start a new login from your terminal.",
                    ),
                    None,
                )
            }
        },
        "deny" => match state
            .forest_client
            .deny_device_login(user_code, user_id)
            .await
        {
            Ok(()) => render_device(&state, &session, user_code, None, Some("denied")),
            Err(e) => {
                tracing::warn!(error = %e, "device login denial failed");
                render_device(
                    &state,
                    &session,
                    user_code,
                    Some("That code wasn't recognised, or has already expired."),
                    None,
                )
            }
        },
        _ => Err(error_page(
            &state,
            StatusCode::BAD_REQUEST,
            "Invalid action",
            "Use the Approve or Deny buttons.",
        )),
    }
}

async fn decide_app_grant(
    state: &AppState,
    session: &Session,
    user_code: &str,
    app: &PendingOAuthDeviceAuthorization,
    form: &DeviceForm,
    approving_ip: &str,
    approving_user_agent: &str,
) -> Result<Response, Response> {
    if !matches!(form.action.as_str(), "approve" | "deny") {
        return Err(error_page(
            state,
            StatusCode::BAD_REQUEST,
            "Invalid action",
            "Use the Approve or Deny buttons.",
        ));
    }
    if form.client_id != app.client_id {
        return render_device_for(
            state,
            session,
            user_code,
            Some(app),
            Some("Check what this code is for before you approve it."),
            None,
        );
    }
    let client = state
        .oauth_apps_client
        .as_ref()
        .expect("an app grant was found through the OAuth client");
    let user_id = &session.user.user_id;
    let outcome = if form.action == "approve" {
        client
            .approve_oauth_device_authorization(
                user_code,
                user_id,
                approving_ip,
                approving_user_agent,
            )
            .await
            .map(|()| "approved")
    } else {
        client
            .deny_oauth_device_authorization(user_code, user_id)
            .await
            .map(|()| "denied")
    };
    match outcome {
        Ok(result) => render_device_for(state, session, user_code, Some(app), None, Some(result)),
        Err(e) => {
            tracing::warn!(error = %e, "app device authorization decision failed");
            render_device_for(
                state,
                session,
                user_code,
                None,
                Some(
                    "That code wasn't recognised, or has already expired. Start a new login from your terminal.",
                ),
                None,
            )
        }
    }
}

fn render_device(
    state: &AppState,
    session: &Session,
    user_code: &str,
    error: Option<&str>,
    result: Option<&str>,
) -> Result<Response, Response> {
    render_device_for(state, session, user_code, None, error, result)
}

fn render_device_for(
    state: &AppState,
    session: &Session,
    user_code: &str,
    app: Option<&PendingOAuthDeviceAuthorization>,
    error: Option<&str>,
    result: Option<&str>,
) -> Result<Response, Response> {
    let title = match app {
        Some(app) => format!("Authorise {}", app.name),
        None => "Authorise Forest CLI".to_string(),
    };
    let html = state
        .templates
        .render(
            "pages/device.html.jinja",
            context! {
                title => title,
                description => "Approve a device login",
                app => app.map(|a| context! {
                    name => &a.name,
                    client_id => &a.client_id,
                    homepage_url => &a.homepage_url,
                    scopes => a.scopes.iter().map(|s| context! {
                        name => s,
                        description => scope_description(s),
                    }).collect::<Vec<_>>(),
                }),
                user => context! { username => &session.user.username },
                current_org => session.user.orgs.first().map(|o| &o.name),
                orgs => session.user.orgs.iter().map(|o| context! {
                    name => &o.name,
                    role => &o.role,
                }).collect::<Vec<_>>(),
                csrf_token => &session.csrf_token,
                user_code => user_code,
                error => error,
                result => result,
            },
        )
        .map_err(|e| internal_error(state, "template error", &e))?;
    Ok(Html(html).into_response())
}
