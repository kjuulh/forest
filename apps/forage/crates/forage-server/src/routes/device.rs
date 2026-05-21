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

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use minijinja::context;
use serde::Deserialize;

use super::{error_page, internal_error};
use crate::auth::{self, Session};
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
    let user_code_prefill = params.user_code.unwrap_or_default();
    render_device(&state, &session, &user_code_prefill, None, None)
}

#[derive(Deserialize)]
struct DeviceForm {
    user_code: String,
    action: String, // "approve" or "deny"
    _csrf: String,
}

async fn device_submit(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<DeviceForm>,
) -> Result<Response, Response> {
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

    match form.action.as_str() {
        "approve" => match state
            .forest_client
            .approve_device_login(
                user_code,
                user_id,
                &approving_ip,
                &approving_user_agent,
            )
            .await
        {
            Ok(()) => render_device(
                &state,
                &session,
                user_code,
                None,
                Some("approved"),
            ),
            Err(e) => {
                tracing::warn!(error = %e, "device login approval failed");
                render_device(
                    &state,
                    &session,
                    user_code,
                    Some("That code wasn't recognised, or has already expired. Start a new login from your terminal."),
                    None,
                )
            }
        },
        "deny" => match state
            .forest_client
            .deny_device_login(user_code, user_id)
            .await
        {
            Ok(()) => render_device(
                &state,
                &session,
                user_code,
                None,
                Some("denied"),
            ),
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

fn render_device(
    state: &AppState,
    session: &Session,
    user_code: &str,
    error: Option<&str>,
    result: Option<&str>,
) -> Result<Response, Response> {
    let html = state
        .templates
        .render(
            "pages/device.html.jinja",
            context! {
                title => "Authorise Forest CLI",
                description => "Approve a device login for the Forest CLI",
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
