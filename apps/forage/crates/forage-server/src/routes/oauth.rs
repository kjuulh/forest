//! Public OAuth 2.0 authorization-server endpoints ("Sign in with Forest").
//!
//! Forage is the public HTTP edge; all validation/issuance is delegated to
//! forest-server over gRPC (the `ForestOAuthApps` client, authenticated as a
//! service account). The browser session is the resource owner.
//!
//! - `GET  /oauth/authorize` — consent screen (session-gated)
//! - `POST /oauth/authorize` — consent decision (CSRF-protected)
//! - `POST /oauth/token`     — code → token exchange (machine-to-machine)
#![allow(clippy::result_large_err)]

use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Json, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use forage_core::platform::{OAuthClientInfo, OAuthFlowError};
use minijinja::context;
use serde::Deserialize;
use serde_json::json;

use super::{error_page, internal_error};
use crate::auth::Session;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/oauth/authorize", get(authorize_page).post(authorize_decision))
        .route("/oauth/token", post(token))
        .route("/oauth/userinfo", get(userinfo).post(userinfo))
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/settings/authorized-apps", get(authorized_apps_page))
        .route(
            "/settings/authorized-apps/{app_id}/revoke",
            post(revoke_authorized_app),
        )
}

// ─── Authorized apps (resource-owner view) ───────────────────────────

async fn authorized_apps_page(
    State(state): State<AppState>,
    session: Session,
) -> Result<Response, Response> {
    let client = require_client(&state).await?;
    let grants = client
        .list_oauth_grants(&session.user.user_id)
        .await
        .map_err(|e| internal_error(&state, "list oauth grants", &e))?;

    let html = state
        .templates
        .render(
            "pages/authorized_apps.html.jinja",
            context! {
                title => "Authorized applications - Forest",
                description => "Apps you've granted access to your account",
                user => context! { username => &session.user.username },
                current_org => session.user.orgs.first().map(|o| &o.name),
                orgs => session.user.orgs.iter().map(|o| context! { name => &o.name, role => &o.role }).collect::<Vec<_>>(),
                csrf_token => &session.csrf_token,
                active_tab => "authorized-apps",
                grants => grants.iter().map(|g| context! {
                    app_id => &g.app_id,
                    name => &g.name,
                    scopes => &g.scopes,
                }).collect::<Vec<_>>(),
            },
        )
        .map_err(|e| internal_error(&state, "template error", &e))?;
    Ok(Html(html).into_response())
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

// ─── Authorize (GET) ─────────────────────────────────────────────────

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

    // Resolve and validate the client + redirect_uri BEFORE trusting either (or
    // before sending an unauthenticated user to login) — on failure we render
    // an on-site error, never redirect (open-redirect / mix-up defence).
    let app = match client.lookup_oauth_client(&q.client_id).await {
        Ok(Some(app)) => app,
        Ok(None) => return Err(invalid_client_page(&state)),
        Err(e) => return Err(internal_error(&state, "lookup oauth client", &e)),
    };
    if !redirect_uri_allowed(&app, &q.redirect_uri) {
        return Err(invalid_redirect_page(&state));
    }

    // From here, redirect_uri is trusted: protocol errors go back to it.
    if q.response_type != "code" {
        return Ok(redirect_error(&q.redirect_uri, "unsupported_response_type", q.state.as_deref()));
    }
    let scopes = match resolve_scopes(&app, &q.scope) {
        Ok(s) => s,
        Err(()) => {
            return Ok(redirect_error(&q.redirect_uri, "invalid_scope", q.state.as_deref()));
        }
    };

    // Authentication. With `prompt=none` we must NOT show UI, so an
    // unauthenticated request returns `login_required` rather than bouncing to
    // the login page (OIDC §3.1.2.6). Otherwise send the user to login,
    // returning to this authorize URL afterwards.
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

    // Remembered consent + OIDC `prompt` (§3.1.2.1):
    //   - prompt=none    → never show UI: auto-approve if already consented for
    //                      these scopes, else error=consent_required.
    //   - prompt=consent → always show the screen (force re-consent).
    //   - default        → auto-approve when prior consent covers the request,
    //                      otherwise show the screen.
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
        "consent" => { /* force the screen below */ }
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
                // Echo the validated request back through the consent form.
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

// ─── Authorize (POST — consent decision) ─────────────────────────────

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

    // Re-validate the client + redirect_uri server-side (never trust the
    // posted hidden fields for the redirect target).
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
        return Ok(redirect_error(&form.redirect_uri, "access_denied", state_param));
    }

    // The displayed consent params must match what was rendered (review #2):
    // reject if the hidden client_id / redirect_uri / scope were tampered.
    let expected =
        consent_binding(&session.csrf_token, &form.client_id, &form.redirect_uri, &form.scope);
    if !consent_binding_valid(&expected, &form.consent_binding) {
        return Err((StatusCode::FORBIDDEN, "consent binding mismatch").into_response());
    }

    let scopes = match resolve_scopes(&app, &form.scope) {
        Ok(s) => s,
        Err(()) => return Ok(redirect_error(&form.redirect_uri, "invalid_scope", state_param)),
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

/// Mint an authorization code for a consented request and 302 back to the
/// client with `code` (+ `state`). On a flow error, redirect with the matching
/// OAuth error. Shared by the explicit consent POST and the remembered-consent
/// auto-approve path.
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
        Err(e) => {
            let oauth_err = match e {
                OAuthFlowError::InvalidScope => "invalid_scope",
                OAuthFlowError::InvalidRequest(_) => "invalid_request",
                _ => "server_error",
            };
            redirect_error(redirect_uri, oauth_err, state)
        }
    }
}

// ─── Token endpoint ──────────────────────────────────────────────────

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
    /// Space-delimited, per RFC 6749 §3.3. Only read for
    /// `client_credentials`; empty means "everything this app has".
    #[serde(default)]
    scope: String,
}

async fn token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    let Some(client) = state.oauth_apps_client.as_ref() else {
        return token_error(StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable");
    };

    // Client credentials may arrive via HTTP Basic (RFC 6749 §2.3.1) or in the
    // body (`client_secret_post`). Basic takes precedence when present.
    let (client_id, client_secret) = match parse_basic_auth(&headers) {
        Some((id, secret)) => (id, secret),
        None => (form.client_id.clone(), form.client_secret.clone()),
    };

    // client_credentials answers with a different shape — no refresh
    // token, no id_token — so it returns straight from here rather than
    // being forced through the user-token response builder below.
    if form.grant_type == "client_credentials" {
        let scopes: Vec<String> = form
            .scope
            .split_whitespace()
            .map(str::to_string)
            .collect();
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

// ─── OIDC discovery ──────────────────────────────────────────────────

async fn discovery(State(state): State<AppState>) -> Response {
    let base = &state.forage_host;
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "userinfo_endpoint": format!("{base}/oauth/userinfo"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token", "client_credentials"],
        "scopes_supported": ["openid", "profile", "email"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
        "id_token_signing_alg_values_supported": ["HS256"],
        "code_challenge_methods_supported": ["S256", "plain"],
        "prompt_values_supported": ["none", "consent"],
        "subject_types_supported": ["public"],
        "claims_supported": ["sub", "iss", "aud", "exp", "iat", "preferred_username", "email"],
    }))
    .into_response()
}

// ─── Userinfo endpoint ───────────────────────────────────────────────

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
            // Build a claims object containing only the granted/non-empty fields.
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
        Err(OAuthFlowError::ServerError(_)) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "server_error" })))
                .into_response()
        }
        Err(_) => unauthorized("invalid_token"),
    }
}

/// RFC 6749 §5.2 error code and its HTTP status.
fn oauth_error_code(e: &OAuthFlowError) -> (StatusCode, &'static str) {
    match e {
        OAuthFlowError::InvalidClient => (StatusCode::UNAUTHORIZED, "invalid_client"),
        OAuthFlowError::InvalidGrant => (StatusCode::BAD_REQUEST, "invalid_grant"),
        OAuthFlowError::InvalidScope => (StatusCode::BAD_REQUEST, "invalid_scope"),
        OAuthFlowError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
        OAuthFlowError::ServerError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
    }
}

/// A 401 with the RFC 6750 `WWW-Authenticate` challenge.
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

// ─── Helpers ─────────────────────────────────────────────────────────

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

/// Exact-match check against the registered redirect URIs.
fn redirect_uri_allowed(app: &OAuthClientInfo, redirect_uri: &str) -> bool {
    app.redirect_uris.iter().any(|u| u == redirect_uri)
}

/// Resolve requested scopes against the app's allowlist. Empty request →
/// all app scopes (RFC 6749 §3.3 default). Any unknown scope → `Err`.
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

fn scope_description(scope: &str) -> &'static str {
    match scope {
        "openid" => "Confirm your identity",
        "profile" => "Your username, account ID and avatar",
        "email" => "Your verified email addresses",
        _ => "",
    }
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.filter(|v| !v.is_empty())
}

/// Bind the consent-critical parameters (client_id, redirect_uri, scope) to the
/// session so they cannot be tampered between the rendered consent screen and
/// the submitted decision. The tag is `HMAC-SHA256(csrf_token, params)`: an
/// attacker editing a hidden field can't forge a matching tag without the
/// session's secret csrf_token. Closes review finding #2 (consent integrity).
pub(crate) fn consent_binding(
    csrf_token: &str,
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = <Hmac<Sha256>>::new_from_slice(csrf_token.as_bytes())
        .expect("hmac accepts any key length");
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

/// Constant-time comparison of two consent tags.
fn consent_binding_valid(expected: &str, presented: &str) -> bool {
    expected.len() == presented.len()
        && expected
            .bytes()
            .zip(presented.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// Parse client credentials from an HTTP Basic `Authorization` header
/// (RFC 6749 §2.3.1). The userid/password are `application/x-www-form-urlencoded`
/// before base64 encoding, so we URL-decode each component after splitting.
/// Returns `None` if there is no usable Basic header.
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
    use super::parse_basic_auth;
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
        // RFC 6749 §2.3.1: components are form-urlencoded before base64.
        let h = basic("cli%40ent:s%2Fcret");
        assert_eq!(
            parse_basic_auth(&h),
            Some(("cli@ent".into(), "s/cret".into()))
        );
    }

    #[test]
    fn empty_secret_is_allowed() {
        let h = basic("public-client:");
        assert_eq!(parse_basic_auth(&h), Some(("public-client".into(), String::new())));
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
        // No colon separator.
        assert_eq!(parse_basic_auth(&basic("nocolon")), None);
        // Empty client_id.
        assert_eq!(parse_basic_auth(&basic(":secret")), None);
    }
}
