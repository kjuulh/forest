use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::state::AppState;

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

    let caller = match client.introspect_client_token(caller_token).await {
        Ok(Some(p)) if p.user.is_none() => p,
        Ok(_) => return error(StatusCode::UNAUTHORIZED, "invalid_token"),
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
        Ok(Some(p)) => {
            let mut body = json!({
                "active": true,
                "token_type": "bearer",
                "client_id": p.app_id,
                "organisation_id": p.organisation_id,
                "scope": p.scopes.join(" "),
            });
            if let Some(user) = p.user {
                let organisations: Vec<_> = user
                    .organisations
                    .into_iter()
                    .map(|o| json!({"id": o.organisation_id, "name": o.name}))
                    .collect();
                body["token_kind"] = json!("user");
                body["sub"] = json!(user.user_id);
                body["username"] = json!(user.username);
                body["organisations"] = json!(organisations);
            }
            Json(body).into_response()
        }
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
