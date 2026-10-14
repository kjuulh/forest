//! `POST /oauth/introspect`: token introspection for resource servers
//! (RFC 7662).
//!
//! A service that accepts forest client-credentials tokens (the data
//! gateway, for project metrics) has to ask whether a token is live and
//! what it may do. Forest answers that through
//! `OAuthAppService/IntrospectClientToken`, which only the service account
//! may call, and that key must not live in every resource server. So Forage,
//! which already holds it, answers on its behalf, the same way it does for
//! `/api/directory/resolve`.
//!
//! The caller authenticates with its OWN client-credentials token, which must
//! carry the `tokens:introspect` scope. The token being asked about goes in
//! the form body (`token=`), per the RFC. An unknown, expired or revoked token
//! is `{"active": false}` and nothing else, so this is no oracle for which
//! tokens exist.

use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::state::AppState;

/// Scope the CALLER's token must carry. Mirrors `SCOPE_TOKENS_INTROSPECT` in
/// Forest, where it grants no gRPC method at all.
const SCOPE_TOKENS_INTROSPECT: &str = "tokens:introspect";

pub fn routes() -> Router<AppState> {
    Router::new().route("/oauth/introspect", post(introspect))
}

#[derive(Deserialize)]
struct IntrospectForm {
    #[serde(default)]
    token: String,
}

async fn introspect(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<IntrospectForm>,
) -> Response {
    let Some(client) = state.oauth_apps_client.as_ref() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable");
    };
    let Some(caller_token) = bearer(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "invalid_token");
    };

    // Authenticate the caller before looking at what it asks about.
    let caller = match client.introspect_client_token(caller_token).await {
        Ok(Some(p)) => p,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "invalid_token"),
        Err(e) => {
            tracing::warn!(?e, "introspect: caller token lookup failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "server_error");
        }
    };
    if !caller.scopes.iter().any(|s| s == SCOPE_TOKENS_INTROSPECT) {
        return error(StatusCode::FORBIDDEN, "insufficient_scope");
    }

    let token = form.token.trim();
    if token.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_request", "error_description": "token is required"})),
        )
            .into_response();
    }

    match client.introspect_client_token(token).await {
        Ok(Some(p)) => Json(json!({
            "active": true,
            "token_type": "bearer",
            "client_id": p.app_id,
            "organisation_id": p.organisation_id,
            "scope": p.scopes.join(" "),
        }))
        .into_response(),
        Ok(None) => Json(json!({"active": false})).into_response(),
        Err(e) => {
            tracing::warn!(?e, app_id = %caller.app_id, "introspect: token lookup failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, "server_error")
        }
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// RFC 6750-shaped error, with the challenge header on 401.
fn error(status: StatusCode, code: &'static str) -> Response {
    if status == StatusCode::UNAUTHORIZED {
        return (
            status,
            [(
                axum::http::header::WWW_AUTHENTICATE,
                format!("Bearer error=\"{code}\""),
            )],
            Json(json!({ "error": code })),
        )
            .into_response();
    }
    (status, Json(json!({ "error": code }))).into_response()
}
