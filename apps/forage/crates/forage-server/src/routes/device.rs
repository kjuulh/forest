//! `/device` — browser side of the forest CLI device-login flow
//! (TASKS/022-device-login.md §1.5).
//!
//! The CLI calls forest-server's `InitiateDeviceLogin` directly, then
//! sends the user here with `?user_code=ABCD-EFGH`. We render an
//! approval form; the POST handler calls `ApproveDeviceLogin` (or
//! `DenyDeviceLogin`) on forest-server using forage's service-account
//! credential — same trust pattern as `OAuthLogin`.
//!
//! Authentication: the GET handler requires a logged-in session. If the
//! user isn't logged in the `Session` extractor redirects to
//! `/login?return_to=/device?user_code=…` and brings them back.
//!
//! The same page approves an OAuth app's device grant (RFC 8628, e.g.
//! `datactl auth login` over SSH). Which kind a code is decides what
//! approving means, so the page asks Forest first: an app's grant is shown
//! with the app and the scopes it gets, and is approved through the OAuth
//! service, which records the consent and later hands the app its own
//! tokens. The form echoes the client_id it showed; a code for any other app
//! (or one typed without seeing it) is shown again instead of approved, so
//! nobody approves an app they were not shown.

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

/// The app grant behind a code, if it is one. A Forest CLI login, an unknown
/// code, or a deployment without the OAuth service is `None`.
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
    action: String, // "approve" or "deny"
    _csrf: String,
    /// The app the page showed for this code; empty when it showed none.
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
        // Defence in depth — the GET handler already shunts users to
        // the "unconfigured" page, but a stale form post must not slip
        // through to forest_client and mint a misleading
        // "code wasn't recognised" error.
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

    // Forward the approver's apparent IP + UA so forest-server's audit
    // log captures who confirmed from where. The ALB sets
    // X-Forwarded-For; if it's absent (local dev) we leave the IP empty
    // rather than wire up `ConnectInfo` (which would need a different
    // axum::serve setup).
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
        // Anything other than approve/deny is a probable bot or tampered
        // form. Treat as a hard 400 to avoid silent acceptance.
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
                // "approved" | "denied" | None — the template uses this
                // to swap the form for a success/cancelled banner.
                result => result,
            },
        )
        .map_err(|e| internal_error(state, "template error", &e))?;
    Ok(Html(html).into_response())
}
