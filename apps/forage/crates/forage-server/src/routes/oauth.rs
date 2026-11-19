#![allow(clippy::result_large_err)]

use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Redirect, Response};
use axum::routing::{get, post};
use forage_core::platform::{OAuthClientInfo, OAuthDeviceToken, OAuthFlowError};
use minijinja::context;
use serde::Deserialize;
use serde_json::json;

use super::{error_page, internal_error};
use crate::auth::Session;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/oauth/authorize",
            get(authorize_page).post(authorize_decision),
        )
        .route("/oauth/token", post(token))
        .route("/oauth/device_authorization", post(device_authorization))
        .route("/oauth/userinfo", get(userinfo).post(userinfo))
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/settings/authorized-apps", get(authorized_apps_page))
        .route(
            "/settings/authorized-apps/{app_id}/revoke",
            post(revoke_authorized_app),
        )
}

async fn authorized_apps_page(
    State(state): State<AppState>,
    session: Session,
) -> Result<Response, Response> {
    let client = require_client(&state).await?;
    let grants = client
        .list_oauth_grants(&session.user.user_id)
        .await
        .map_err(|e| internal_error(&state, "list oauth grants", &e))?;

    let page = crate::views::account::AuthorizedAppsPage {
        chrome: crate::views::account::chrome(
            &state,
            &session,
            crate::views::account::first_org(&session),
            "Authorized applications - Forest",
            "Apps you've granted access to your account",
        ),
        csrf_token: session.csrf_token.clone(),
        grants,
    };
    Ok(Html(crate::views::pages::authorized_apps::render(&page)).into_response())
}

#[derive(Deserialize)]
struct RevokeForm {
    #[serde(rename = "_csrf")]
    csrf: String,
}

async fn revoke_authorized_app(
    State(state): State<AppState>,
    session: Session,
    axum::extract::Path(app_id): axum::extract::Path<String>,
    Form(form): Form<RevokeForm>,
) -> Result<Response, Response> {
    let client = require_client(&state).await?;
    if session.csrf_token != form.csrf {
        return Err((StatusCode::FORBIDDEN, "CSRF token mismatch").into_response());
    }
    client
        .revoke_oauth_grant(&session.user.user_id, &app_id)
        .await
        .map_err(|e| internal_error(&state, "revoke oauth grant", &e))?;
    Ok(Redirect::to("/settings/authorized-apps").into_response())
}

#[derive(Deserialize)]
struct AuthorizeQuery {
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    redirect_uri: String,
    #[serde(default)]
    response_type: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    code_challenge: Option<String>,
    #[serde(default)]
    code_challenge_method: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

async fn authorize_page(
    State(state): State<AppState>,
    maybe_session: crate::auth::MaybeSession,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    Query(q): Query<AuthorizeQuery>,
) -> Result<Response, Response> {
    let client = require_client(&state).await?;

    let app = match client.lookup_oauth_client(&q.client_id).await {
        Ok(Some(app)) => app,
        Ok(None) => return Err(invalid_client_page(&state)),
        Err(e) => return Err(internal_error(&state, "lookup oauth client", &e)),
    };
    if !redirect_uri_allowed(&app, &q.redirect_uri) {
        return Err(invalid_redirect_page(&state));
    }

    if q.response_type != "code" {
        return Ok(redirect_error(
            &q.redirect_uri,
            "unsupported_response_type",
            q.state.as_deref(),
        ));
    }
    if app.public
        && (non_empty(q.code_challenge.as_deref()).is_none()
            || q.code_challenge_method.as_deref() != Some("S256"))
    {
        return Ok(redirect_error_described(
            &q.redirect_uri,
            "invalid_request",
            PUBLIC_CLIENT_PKCE_REQUIRED,
            q.state.as_deref(),
        ));
    }
    let scopes = match resolve_scopes(&app, &q.scope) {
        Ok(s) => s,
        Err(()) => {
            return Ok(redirect_error(
                &q.redirect_uri,
                "invalid_scope",
                q.state.as_deref(),
            ));
        }
    };

    let session = match maybe_session.session {
        Some(s) => s,
        None => {
            return Ok(if q.prompt.as_deref() == Some("none") {
                redirect_error(&q.redirect_uri, "login_required", q.state.as_deref())
            } else {
                crate::auth::login_redirect(&uri).into_response()
            });
        }
    };

    let prior = client
        .get_oauth_consent(&q.client_id, &session.user.user_id)
        .await
        .unwrap_or_default();
    let covered = !prior.is_empty() && scopes.iter().all(|s| prior.contains(s));
    let prompt = q.prompt.as_deref().unwrap_or("");

    let auto_approve = || async {
        issue_code_redirect(
            client,
            &session.user.user_id,
            &q.client_id,
            &q.redirect_uri,
            &scopes,
            q.state.as_deref(),
            non_empty(q.code_challenge.as_deref()),
            non_empty(q.code_challenge_method.as_deref()),
            non_empty(q.nonce.as_deref()),
        )
        .await
    };

    match prompt {
        "none" => {
            return Ok(if covered {
                auto_approve().await
            } else {
                redirect_error(&q.redirect_uri, "consent_required", q.state.as_deref())
            });
        }
        "consent" => {}
        _ if covered => return Ok(auto_approve().await),
        _ => {}
    }

    let html = state
        .templates
        .render(
            "pages/oauth_consent.html.jinja",
            context! {
                title => format!("Authorize {} - Forest", app.name),
                description => "Authorize application",
                user => context! {
                    username => &session.user.username,
                    user_id => &session.user.user_id,
                },
                csrf_token => &session.csrf_token,
                app_name => &app.name,
                app_homepage => &app.homepage_url,
                scopes => scopes.iter().map(|s| context! {
                    name => s,
                    description => scope_description(s),
                }).collect::<Vec<_>>(),
                client_id => &q.client_id,
                redirect_uri => &q.redirect_uri,
                scope => scopes.join(" "),
                consent_binding => consent_binding(
                    &session.csrf_token,
                    &q.client_id,
                    &q.redirect_uri,
                    &scopes.join(" "),
                ),
                state => q.state.clone().unwrap_or_default(),
                code_challenge => q.code_challenge.clone().unwrap_or_default(),
                code_challenge_method => q.code_challenge_method.clone().unwrap_or_default(),
                nonce => q.nonce.clone().unwrap_or_default(),
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;
    Ok(Html(html).into_response())
}

#[derive(Deserialize)]
struct ConsentForm {
    #[serde(rename = "_csrf")]
    csrf: String,
    action: String,
    client_id: String,
    redirect_uri: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    code_challenge: Option<String>,
    #[serde(default)]
    code_challenge_method: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    consent_binding: String,
}

async fn authorize_decision(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<ConsentForm>,
) -> Result<Response, Response> {
    let client = require_client(&state).await?;
    if session.csrf_token != form.csrf {
        return Err((StatusCode::FORBIDDEN, "CSRF token mismatch").into_response());
    }

    let app = match client.lookup_oauth_client(&form.client_id).await {
        Ok(Some(app)) => app,
        Ok(None) => return Err(invalid_client_page(&state)),
        Err(e) => return Err(internal_error(&state, "lookup oauth client", &e)),
    };
    if !redirect_uri_allowed(&app, &form.redirect_uri) {
        return Err(invalid_redirect_page(&state));
    }

    let state_param = form.state.as_deref();

    if form.action != "approve" {
        return Ok(redirect_error(
            &form.redirect_uri,
            "access_denied",
            state_param,
        ));
    }

    let expected = consent_binding(
        &session.csrf_token,
        &form.client_id,
        &form.redirect_uri,
        &form.scope,
    );
    if !consent_binding_valid(&expected, &form.consent_binding) {
        return Err((StatusCode::FORBIDDEN, "consent binding mismatch").into_response());
    }

    let scopes = match resolve_scopes(&app, &form.scope) {
        Ok(s) => s,
        Err(()) => {
            return Ok(redirect_error(
                &form.redirect_uri,
                "invalid_scope",
                state_param,
            ));
        }
    };

    Ok(issue_code_redirect(
        client,
        &session.user.user_id,
        &form.client_id,
        &form.redirect_uri,
        &scopes,
        state_param,
        non_empty(form.code_challenge.as_deref()),
        non_empty(form.code_challenge_method.as_deref()),
        non_empty(form.nonce.as_deref()),
    )
    .await)
}

#[allow(clippy::too_many_arguments)]
async fn issue_code_redirect(
    client: &std::sync::Arc<dyn forage_core::platform::ForestOAuthApps>,
    user_id: &str,
    client_id: &str,
    redirect_uri: &str,
    scopes: &[String],
    state: Option<&str>,
    code_challenge: Option<&str>,
    code_challenge_method: Option<&str>,
    nonce: Option<&str>,
) -> Response {
    match client
        .create_oauth_authorization_code(
            client_id,
            user_id,
            redirect_uri,
            scopes,
            code_challenge,
            code_challenge_method,
            nonce,
        )
        .await
    {
        Ok(code) => {
            let mut params = vec![("code", code.as_str())];
            if let Some(s) = state {
                params.push(("state", s));
            }
            Redirect::to(&append_query(redirect_uri, &params)).into_response()
        }
        Err(OAuthFlowError::InvalidRequest(msg)) => match msg.strip_prefix("invalid_request: ") {
            Some(description) => {
                redirect_error_described(redirect_uri, "invalid_request", description, state)
            }
            None => redirect_error(redirect_uri, "invalid_request", state),
        },
        Err(e) => {
            let oauth_err = match e {
                OAuthFlowError::InvalidScope => "invalid_scope",
                _ => "server_error",
            };
            redirect_error(redirect_uri, oauth_err, state)
        }
    }
}

#[derive(Deserialize)]
struct TokenForm {
    #[serde(default)]
    grant_type: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: String,
    #[serde(default)]
    code: String,
    #[serde(default)]
    redirect_uri: String,
    #[serde(default)]
    code_verifier: Option<String>,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    device_code: String,
}

pub(crate) const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

async fn token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    let Some(client) = state.oauth_apps_client.as_ref() else {
        return token_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable");
    };

    let (client_id, client_secret) = match parse_basic_auth(&headers) {
        Some((id, secret)) => (id, secret),
        None => (form.client_id.clone(), form.client_secret.clone()),
    };

    if form.grant_type == "client_credentials" {
        let scopes: Vec<String> = form.scope.split_whitespace().map(str::to_string).collect();
        return match client
            .issue_client_credentials_token(&client_id, &client_secret, &scopes)
            .await
        {
            Ok(t) => Json(json!({
                "access_token": t.access_token,
                "token_type": t.token_type,
                "expires_in": t.expires_in_seconds,
                "scope": t.scopes.join(" "),
            }))
            .into_response(),
            Err(e) => {
                let (status, code) = oauth_error_code(&e);
                token_error(status, code)
            }
        };
    }

    if form.grant_type == DEVICE_CODE_GRANT {
        if form.device_code.is_empty() {
            return token_error(StatusCode::BAD_REQUEST, "invalid_request");
        }
        return match client
            .exchange_oauth_device_code(&client_id, &client_secret, &form.device_code)
            .await
        {
            Ok(OAuthDeviceToken::Issued(tokens)) => Json(json!({
                "access_token": tokens.access_token,
                "token_type": tokens.token_type,
                "expires_in": tokens.expires_in_seconds,
                "refresh_token": tokens.refresh_token,
                "scope": tokens.scopes.join(" "),
            }))
            .into_response(),
            Ok(OAuthDeviceToken::AuthorizationPending) => {
                token_error(StatusCode::BAD_REQUEST, "authorization_pending")
            }
            Ok(OAuthDeviceToken::SlowDown) => token_error(StatusCode::BAD_REQUEST, "slow_down"),
            Ok(OAuthDeviceToken::AccessDenied) => {
                token_error(StatusCode::BAD_REQUEST, "access_denied")
            }
            Ok(OAuthDeviceToken::ExpiredToken) => {
                token_error(StatusCode::BAD_REQUEST, "expired_token")
            }
            Err(e) => device_flow_error(&e),
        };
    }

    let result = match form.grant_type.as_str() {
        "authorization_code" => {
            client
                .exchange_oauth_code(
                    &client_id,
                    &client_secret,
                    &form.code,
                    &form.redirect_uri,
                    non_empty(form.code_verifier.as_deref()),
                )
                .await
        }
        "refresh_token" => {
            client
                .refresh_oauth_token(&client_id, &client_secret, &form.refresh_token)
                .await
        }
        _ => return token_error(StatusCode::BAD_REQUEST, "unsupported_grant_type"),
    };

    match result {
        Ok(tokens) => {
            let mut body = serde_json::Map::new();
            body.insert("access_token".into(), json!(tokens.access_token));
            body.insert("token_type".into(), json!(tokens.token_type));
            body.insert("expires_in".into(), json!(tokens.expires_in_seconds));
            body.insert("refresh_token".into(), json!(tokens.refresh_token));
            body.insert("scope".into(), json!(tokens.scopes.join(" ")));
            if let Some(id_token) = tokens.id_token {
                body.insert("id_token".into(), json!(id_token));
            }
            Json(serde_json::Value::Object(body)).into_response()
        }
        Err(e) => {
            let (status, code) = oauth_error_code(&e);
            token_error(status, code)
        }
    }
}

#[derive(Deserialize)]
struct DeviceAuthorizationForm {
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: String,
    #[serde(default)]
    scope: String,
}

async fn device_authorization(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<DeviceAuthorizationForm>,
) -> Response {
    let Some(client) = state.oauth_apps_client.as_ref() else {
        return token_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable");
    };
    let (client_id, client_secret) = match parse_basic_auth(&headers) {
        Some((id, secret)) => (id, secret),
        None => (form.client_id, form.client_secret),
    };
    if client_id.is_empty() {
        return token_error(StatusCode::UNAUTHORIZED, "invalid_client");
    }
    let scopes: Vec<String> = form.scope.split_whitespace().map(str::to_string).collect();

    match client
        .initiate_oauth_device_authorization(&client_id, &client_secret, &scopes)
        .await
    {
        Ok(grant) => Json(json!({
            "device_code": grant.device_code,
            "user_code": grant.user_code,
            "verification_uri": grant.verification_uri,
            "verification_uri_complete": grant.verification_uri_complete,
            "expires_in": grant.expires_in_seconds,
            "interval": grant.interval_seconds,
        }))
        .into_response(),
        Err(e) => device_flow_error(&e),
    }
}

fn device_flow_error(e: &OAuthFlowError) -> Response {
    match e {
        OAuthFlowError::InvalidRequest(msg) if msg == "unsupported_grant_type" => {
            token_error(StatusCode::BAD_REQUEST, "unauthorized_client")
        }
        e => {
            let (status, code) = oauth_error_code(e);
            token_error(status, code)
        }
    }
}

async fn discovery(State(state): State<AppState>) -> Response {
    let base = &state.forage_host;
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "device_authorization_endpoint": format!("{base}/oauth/device_authorization"),
        "userinfo_endpoint": format!("{base}/oauth/userinfo"),
        "response_types_supported": ["code"],
        "grant_types_supported": [
            "authorization_code",
            "refresh_token",
            "client_credentials",
            DEVICE_CODE_GRANT
        ],
        "scopes_supported": [
            "openid",
            "profile",
            "email",
            "components:publish",
            "tokens:introspect",
            "datagateway:apply",
            "datagateway:read"
        ],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post", "none"],
        "id_token_signing_alg_values_supported": ["HS256"],
        "code_challenge_methods_supported": ["S256", "plain"],
        "prompt_values_supported": ["none", "consent"],
        "subject_types_supported": ["public"],
        "claims_supported": ["sub", "iss", "aud", "exp", "iat", "preferred_username", "email"],
    }))
    .into_response()
}

async fn userinfo(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(client) = state.oauth_apps_client.as_ref() else {
        return unauthorized("temporarily_unavailable");
    };

    let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return unauthorized("invalid_token");
    };

    match client.oauth_userinfo(token).await {
        Ok(info) => {
            let mut claims = serde_json::Map::new();
            claims.insert("sub".into(), json!(info.sub));
            if let Some(u) = info.username {
                claims.insert("username".into(), json!(u));
            }
            if let Some(p) = info.profile_picture_url {
                claims.insert("profile_picture_url".into(), json!(p));
            }
            if let Some(e) = info.email {
                claims.insert("email".into(), json!(e));
            }
            if !info.emails.is_empty() {
                claims.insert("emails".into(), json!(info.emails));
            }
            Json(serde_json::Value::Object(claims)).into_response()
        }
        Err(OAuthFlowError::ServerError(_)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "server_error" })),
        )
            .into_response(),
        Err(_) => unauthorized("invalid_token"),
    }
}

fn oauth_error_code(e: &OAuthFlowError) -> (StatusCode, &'static str) {
    match e {
        OAuthFlowError::InvalidClient => (StatusCode::UNAUTHORIZED, "invalid_client"),
        OAuthFlowError::InvalidGrant => (StatusCode::BAD_REQUEST, "invalid_grant"),
        OAuthFlowError::InvalidScope => (StatusCode::BAD_REQUEST, "invalid_scope"),
        OAuthFlowError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
        OAuthFlowError::ServerError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    }
}

fn unauthorized(error: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(
            axum::http::header::WWW_AUTHENTICATE,
            format!("Bearer error=\"{error}\""),
        )],
        Json(json!({ "error": error })),
    )
        .into_response()
}

async fn require_client(
    state: &AppState,
) -> Result<&std::sync::Arc<dyn forage_core::platform::ForestOAuthApps>, Response> {
    state.oauth_apps_client.as_ref().ok_or_else(|| {
        error_page(
            state,
            StatusCode::SERVICE_UNAVAILABLE,
            "Not available",
            "OAuth is not configured on this server.",
        )
    })
}

fn redirect_uri_allowed(app: &OAuthClientInfo, redirect_uri: &str) -> bool {
    app.redirect_uris
        .iter()
        .any(|u| redirect_uri_matches(u, redirect_uri))
}

pub(crate) fn redirect_uri_matches(registered: &str, requested: &str) -> bool {
    if registered == requested {
        return true;
    }
    match (loopback_parts(registered), loopback_parts(requested)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

fn loopback_parts(uri: &str) -> Option<(&str, String)> {
    let rest = uri.strip_prefix("http://")?;
    if rest.contains('#') {
        return None;
    }
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(split);
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if host != "127.0.0.1" && host != "localhost" {
        return None;
    }
    if let Some(port) = port
        && (port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let tail = if tail.starts_with('/') {
        tail.to_string()
    } else {
        format!("/{tail}")
    };
    Some((host, tail))
}

const PUBLIC_CLIENT_PKCE_REQUIRED: &str =
    "a public client must use PKCE with code_challenge_method S256";

fn resolve_scopes(app: &OAuthClientInfo, requested: &str) -> Result<Vec<String>, ()> {
    let requested: Vec<&str> = requested.split_whitespace().collect();
    if requested.is_empty() {
        return Ok(app.scopes.clone());
    }
    let mut out = Vec::new();
    for s in requested {
        if !app.scopes.iter().any(|a| a == s) {
            return Err(());
        }
        if !out.contains(&s.to_string()) {
            out.push(s.to_string());
        }
    }
    Ok(out)
}

pub(crate) fn scope_description(scope: &str) -> &'static str {
    match scope {
        "openid" => "Confirm your identity",
        "profile" => "Your username, account ID and avatar",
        "email" => "Your verified email addresses",
        "components:publish" => "Publish Forest components from CI",
        "tokens:introspect" => "Check other apps' tokens (for a service that accepts them)",
        "datagateway:apply" => "Define data gateway project metrics",
        "datagateway:read" => "Query data gateway project metrics",
        _ => "",
    }
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.filter(|v| !v.is_empty())
}

pub(crate) fn consent_binding(
    csrf_token: &str,
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac =
        <Hmac<Sha256>>::new_from_slice(csrf_token.as_bytes()).expect("hmac accepts any key length");
    mac.update(client_id.as_bytes());
    mac.update(b"\x00");
    mac.update(redirect_uri.as_bytes());
    mac.update(b"\x00");
    mac.update(scope.as_bytes());
    let digest = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn consent_binding_valid(expected: &str, presented: &str) -> bool {
    expected.len() == presented.len()
        && expected
            .bytes()
            .zip(presented.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn parse_basic_auth(headers: &HeaderMap) -> Option<(String, String)> {
    use base64::Engine;
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Basic ")?
        .trim();
    let decoded = base64::engine::general_purpose::STANDARD.decode(raw).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (id, secret) = decoded.split_once(':')?;
    let id = urlencoding::decode(id).ok()?.into_owned();
    let secret = urlencoding::decode(secret).ok()?.into_owned();
    if id.is_empty() {
        return None;
    }
    Some((id, secret))
}

fn append_query(base: &str, params: &[(&str, &str)]) -> String {
    let mut out = base.to_string();
    let mut first = !base.contains('?');
    for (k, v) in params {
        out.push(if first { '?' } else { '&' });
        first = false;
        out.push_str(k);
        out.push('=');
        out.push_str(&urlencoding::encode(v));
    }
    out
}

fn redirect_error(redirect_uri: &str, error: &str, state: Option<&str>) -> Response {
    let mut params = vec![("error", error)];
    if let Some(s) = state {
        params.push(("state", s));
    }
    Redirect::to(&append_query(redirect_uri, &params)).into_response()
}

fn redirect_error_described(
    redirect_uri: &str,
    error: &str,
    description: &str,
    state: Option<&str>,
) -> Response {
    let mut params = vec![("error", error), ("error_description", description)];
    if let Some(s) = state {
        params.push(("state", s));
    }
    Redirect::to(&append_query(redirect_uri, &params)).into_response()
}

fn token_error(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "error": error }))).into_response()
}

fn invalid_client_page(state: &AppState) -> Response {
    error_page(
        state,
        StatusCode::BAD_REQUEST,
        "Invalid client",
        "The OAuth client is not recognised. Contact the application that sent you here.",
    )
}

fn invalid_redirect_page(state: &AppState) -> Response {
    error_page(
        state,
        StatusCode::BAD_REQUEST,
        "Invalid redirect URI",
        "The redirect URI is not registered for this application.",
    )
}

#[cfg(test)]
mod tests {
    use super::{parse_basic_auth, redirect_uri_matches};

    #[test]
    fn a_registered_loopback_redirect_matches_any_port() {
        for registered in [
            "http://127.0.0.1/callback",
            "http://127.0.0.1:8080/callback",
        ] {
            assert!(redirect_uri_matches(
                registered,
                "http://127.0.0.1:53817/callback"
            ));
            assert!(redirect_uri_matches(
                registered,
                "http://127.0.0.1/callback"
            ));
        }
        assert!(redirect_uri_matches(
            "http://localhost/cb?x=1",
            "http://localhost:4000/cb?x=1"
        ));
    }

    #[test]
    fn a_loopback_redirect_still_needs_the_same_host_path_and_query() {
        let registered = "http://127.0.0.1/callback";
        assert!(!redirect_uri_matches(
            registered,
            "http://127.0.0.1:53817/other"
        ));
        assert!(!redirect_uri_matches(
            registered,
            "http://localhost:53817/callback"
        ));
        assert!(!redirect_uri_matches(
            registered,
            "http://127.0.0.1:53817/callback?a=b"
        ));
        assert!(!redirect_uri_matches(
            registered,
            "http://127.0.0.1.evil.example/callback"
        ));
        assert!(!redirect_uri_matches(
            registered,
            "http://127.0.0.1@evil.example/callback"
        ));
    }

    #[test]
    fn only_http_on_a_loopback_host_ignores_the_port() {
        assert!(!redirect_uri_matches(
            "https://127.0.0.1/callback",
            "https://127.0.0.1:53817/callback"
        ));
        assert!(!redirect_uri_matches(
            "https://app.example:8443/callback",
            "https://app.example:9443/callback"
        ));
        assert!(!redirect_uri_matches(
            "http://app.example/callback",
            "http://app.example:8080/callback"
        ));
    }
    use axum::http::{HeaderMap, HeaderValue};
    use base64::Engine;

    fn basic(creds: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        let enc = base64::engine::general_purpose::STANDARD.encode(creds);
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {enc}")).unwrap(),
        );
        h
    }

    #[test]
    fn parses_plain_basic_credentials() {
        let h = basic("my-client:my-secret");
        assert_eq!(
            parse_basic_auth(&h),
            Some(("my-client".into(), "my-secret".into()))
        );
    }

    #[test]
    fn url_decodes_components() {
        let h = basic("cli%40ent:s%2Fcret");
        assert_eq!(
            parse_basic_auth(&h),
            Some(("cli@ent".into(), "s/cret".into()))
        );
    }

    #[test]
    fn empty_secret_is_allowed() {
        let h = basic("public-client:");
        assert_eq!(
            parse_basic_auth(&h),
            Some(("public-client".into(), String::new()))
        );
    }

    #[test]
    fn missing_or_non_basic_header_is_none() {
        assert_eq!(parse_basic_auth(&HeaderMap::new()), None);
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer abc"),
        );
        assert_eq!(parse_basic_auth(&h), None);
    }

    #[test]
    fn malformed_basic_is_none() {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic !!!notbase64!!!"),
        );
        assert_eq!(parse_basic_auth(&h), None);
        assert_eq!(parse_basic_auth(&basic("nocolon")), None);
        assert_eq!(parse_basic_auth(&basic(":secret")), None);
    }
}
