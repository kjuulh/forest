//! Route tests for the public OAuth endpoints (M2):
//! /oauth/authorize (consent) and /oauth/token (code exchange).
//!
//! The forest-server OAuth client is mocked in-memory; these cover the Forage
//! HTTP surface — consent rendering, open-redirect defence, CSRF, the consent
//! decision redirect, and the token-endpoint JSON contract.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

const CID: &str = "forest_oa_testclient";
const REDIRECT: &str = "https://app.example/cb";

fn seeded() -> Arc<MockOAuthAppsClient> {
    let oauth = Arc::new(MockOAuthAppsClient::new());
    oauth.seed_app(
        CID,
        vec![REDIRECT.into()],
        vec!["profile".into(), "email".into()],
    );
    oauth
}

async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

fn location(resp: &axum::response::Response) -> String {
    resp.headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

// ─── Authorize (GET) ─────────────────────────────────────────────────

#[tokio::test]
async fn authorize_renders_consent_screen() {
    let (state, sessions) = test_state_with_oauth_apps(seeded());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let uri = format!(
        "/oauth/authorize?client_id={CID}&redirect_uri={}&response_type=code&scope=profile&state=xyz",
        urlencoding::encode(REDIRECT)
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    assert!(text.contains("Authorize Seeded App"));
    assert!(text.contains("profile"));
}

#[tokio::test]
async fn authorize_unknown_client_renders_error_not_redirect() {
    let (state, sessions) = test_state_with_oauth_apps(seeded());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let uri = format!(
        "/oauth/authorize?client_id=does_not_exist&redirect_uri={}&response_type=code",
        urlencoding::encode(REDIRECT)
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Open-redirect defence: an unknown client is an on-site error, never a
    // redirect.
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(resp.headers().get("location").is_none());
}

#[tokio::test]
async fn authorize_unregistered_redirect_renders_error() {
    let (state, sessions) = test_state_with_oauth_apps(seeded());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let uri = format!(
        "/oauth/authorize?client_id={CID}&redirect_uri={}&response_type=code",
        urlencoding::encode("https://evil.example/cb")
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(resp.headers().get("location").is_none());
}

#[tokio::test]
async fn authorize_unauthenticated_redirects_to_login() {
    let (state, _sessions) = test_state_with_oauth_apps(seeded());
    let app = build_router(state);

    let uri = format!(
        "/oauth/authorize?client_id={CID}&redirect_uri={}&response_type=code",
        urlencoding::encode(REDIRECT)
    );
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(resp.status().is_redirection());
    assert!(location(&resp).starts_with("/login"));
}

#[tokio::test]
async fn authorize_bad_scope_redirects_to_client_with_error() {
    let (state, sessions) = test_state_with_oauth_apps(seeded());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let uri = format!(
        "/oauth/authorize?client_id={CID}&redirect_uri={}&response_type=code&scope=admin",
        urlencoding::encode(REDIRECT)
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // redirect_uri is valid, so a bad scope goes back to the client.
    assert!(resp.status().is_redirection());
    let loc = location(&resp);
    assert!(loc.starts_with(REDIRECT));
    assert_eq!(query_param(&loc, "error").as_deref(), Some("invalid_scope"));
}

#[tokio::test]
async fn authorize_auto_approves_when_prior_consent_covers_scopes() {
    let oauth = seeded();
    oauth.seed_consent(CID, vec!["profile".into(), "email".into()]);
    let (state, sessions) = test_state_with_oauth_apps(oauth);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    // Requested scope is covered by the prior consent → skip the screen.
    let uri = format!(
        "/oauth/authorize?client_id={CID}&redirect_uri={}&response_type=code&scope=profile&state=s1",
        urlencoding::encode(REDIRECT)
    );
    let resp = app
        .oneshot(get(&uri, &cookie))
        .await
        .unwrap();
    assert!(resp.status().is_redirection(), "auto-approved → redirect");
    let loc = location(&resp);
    assert!(loc.starts_with(REDIRECT));
    assert!(query_param(&loc, "code").is_some());
    assert_eq!(query_param(&loc, "state").as_deref(), Some("s1"));
}

#[tokio::test]
async fn authorize_prompts_when_prior_consent_is_insufficient() {
    let oauth = seeded();
    // Prior consent only covers `profile`; request asks for `email` too.
    oauth.seed_consent(CID, vec!["profile".into()]);
    let (state, sessions) = test_state_with_oauth_apps(oauth);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let uri = format!(
        "/oauth/authorize?client_id={CID}&redirect_uri={}&response_type=code&scope=profile%20email",
        urlencoding::encode(REDIRECT)
    );
    let resp = app.oneshot(get(&uri, &cookie)).await.unwrap();
    // New scope not yet consented → show the screen (200), not a redirect.
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_text(resp).await.contains("Authorize Seeded App"));
}

#[tokio::test]
async fn prompt_none_unauthenticated_errors_login_required() {
    // No session cookie. With prompt=none we must not bounce to login — return
    // login_required to the client (OIDC §3.1.2.6).
    let (state, _sessions) = test_state_with_oauth_apps(seeded());
    let app = build_router(state);

    let uri = format!(
        "/oauth/authorize?client_id={CID}&redirect_uri={}&response_type=code&scope=profile&prompt=none&state=s",
        urlencoding::encode(REDIRECT)
    );
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(resp.status().is_redirection());
    let loc = location(&resp);
    assert!(loc.starts_with(REDIRECT));
    assert_eq!(query_param(&loc, "error").as_deref(), Some("login_required"));
}

#[tokio::test]
async fn prompt_none_auto_approves_when_consented() {
    let oauth = seeded();
    oauth.seed_consent(CID, vec!["profile".into()]);
    let (state, sessions) = test_state_with_oauth_apps(oauth);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let uri = format!(
        "/oauth/authorize?client_id={CID}&redirect_uri={}&response_type=code&scope=profile&prompt=none&state=s",
        urlencoding::encode(REDIRECT)
    );
    let resp = app.oneshot(get(&uri, &cookie)).await.unwrap();
    assert!(resp.status().is_redirection());
    assert!(query_param(&location(&resp), "code").is_some());
}

#[tokio::test]
async fn prompt_none_without_consent_errors_consent_required() {
    let (state, sessions) = test_state_with_oauth_apps(seeded());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let uri = format!(
        "/oauth/authorize?client_id={CID}&redirect_uri={}&response_type=code&scope=profile&prompt=none&state=s",
        urlencoding::encode(REDIRECT)
    );
    let resp = app.oneshot(get(&uri, &cookie)).await.unwrap();
    // No UI shown — error returned to the client per OIDC.
    assert!(resp.status().is_redirection());
    let loc = location(&resp);
    assert_eq!(query_param(&loc, "error").as_deref(), Some("consent_required"));
    assert_eq!(query_param(&loc, "state").as_deref(), Some("s"));
}

#[tokio::test]
async fn prompt_consent_forces_screen_despite_prior_consent() {
    let oauth = seeded();
    oauth.seed_consent(CID, vec!["profile".into()]);
    let (state, sessions) = test_state_with_oauth_apps(oauth);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let uri = format!(
        "/oauth/authorize?client_id={CID}&redirect_uri={}&response_type=code&scope=profile&prompt=consent",
        urlencoding::encode(REDIRECT)
    );
    let resp = app.oneshot(get(&uri, &cookie)).await.unwrap();
    // Forced re-consent: the screen is rendered even though prior consent exists.
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_text(resp).await.contains("Authorize Seeded App"));
}

// ─── Consent decision (POST) ─────────────────────────────────────────

fn consent_body(action: &str) -> String {
    let binding = crate::routes::oauth::consent_binding("test-csrf", CID, REDIRECT, "profile");
    format!(
        "_csrf=test-csrf&action={action}&client_id={CID}&redirect_uri={}&scope=profile&state=xyz&consent_binding={binding}",
        urlencoding::encode(REDIRECT)
    )
}

fn post(uri: &str, cookie: &str, body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("cookie", cookie)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

fn get(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn consent_approve_redirects_with_code_and_state() {
    let (state, sessions) = test_state_with_oauth_apps(seeded());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let resp = app
        .oneshot(post("/oauth/authorize", &cookie, consent_body("approve")))
        .await
        .unwrap();
    assert!(resp.status().is_redirection());
    let loc = location(&resp);
    assert!(loc.starts_with(REDIRECT));
    assert!(query_param(&loc, "code").is_some());
    assert_eq!(query_param(&loc, "state").as_deref(), Some("xyz"));
}

#[tokio::test]
async fn consent_deny_redirects_with_access_denied() {
    let (state, sessions) = test_state_with_oauth_apps(seeded());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let resp = app
        .oneshot(post("/oauth/authorize", &cookie, consent_body("deny")))
        .await
        .unwrap();
    assert!(resp.status().is_redirection());
    let loc = location(&resp);
    assert_eq!(query_param(&loc, "error").as_deref(), Some("access_denied"));
    assert_eq!(query_param(&loc, "state").as_deref(), Some("xyz"));
}

#[tokio::test]
async fn consent_with_tampered_scope_is_rejected() {
    let (state, sessions) = test_state_with_oauth_apps(seeded());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    // The user was shown (and the binding covers) only `profile`, but the
    // submitted scope is widened to `profile email`. The binding no longer
    // matches → reject (review finding #2).
    let binding = crate::routes::oauth::consent_binding("test-csrf", CID, REDIRECT, "profile");
    let body = format!(
        "_csrf=test-csrf&action=approve&client_id={CID}&redirect_uri={}&scope=profile+email&consent_binding={binding}",
        urlencoding::encode(REDIRECT)
    );
    let resp = app
        .oneshot(post("/oauth/authorize", &cookie, body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn consent_bad_csrf_rejected() {
    let (state, sessions) = test_state_with_oauth_apps(seeded());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let body = consent_body("approve").replace("_csrf=test-csrf", "_csrf=wrong");
    let resp = app
        .oneshot(post("/oauth/authorize", &cookie, body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ─── Token endpoint ──────────────────────────────────────────────────

#[tokio::test]
async fn token_exchange_returns_bearer_json() {
    let (state, sessions) = test_state_with_oauth_apps(seeded());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    // Approve to obtain a real code from the mock.
    let resp = app
        .clone()
        .oneshot(post("/oauth/authorize", &cookie, consent_body("approve")))
        .await
        .unwrap();
    let code = query_param(&location(&resp), "code").expect("code");

    let body = format!(
        "grant_type=authorization_code&client_id={CID}&client_secret=forest_oas_secret&code={code}&redirect_uri={}",
        urlencoding::encode(REDIRECT)
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(json["access_token"].as_str().unwrap().starts_with("forest_oat_"));
    assert_eq!(json["token_type"], "bearer");
    assert_eq!(json["scope"], "profile");
}

#[tokio::test]
async fn token_refresh_grant_returns_new_tokens() {
    let (state, _sessions) = test_state_with_oauth_apps(seeded());
    let app = build_router(state);

    let body = format!(
        "grant_type=refresh_token&client_id={CID}&client_secret=s&refresh_token=forest_ort_mocktoken"
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(json["access_token"], "forest_oat_refreshed");
    assert_eq!(json["refresh_token"], "forest_ort_refreshed");
}

#[tokio::test]
async fn token_refresh_grant_with_bad_token_is_invalid_grant() {
    let (state, _sessions) = test_state_with_oauth_apps(seeded());
    let app = build_router(state);

    let body = format!(
        "grant_type=refresh_token&client_id={CID}&client_secret=s&refresh_token=bogus"
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(json["error"], "invalid_grant");
}

#[tokio::test]
async fn token_accepts_http_basic_client_auth() {
    let (state, sessions) = test_state_with_oauth_apps(seeded());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    // Obtain a code via consent.
    let resp = app
        .clone()
        .oneshot(post("/oauth/authorize", &cookie, consent_body("approve")))
        .await
        .unwrap();
    let code = query_param(&location(&resp), "code").expect("code");

    // Credentials via HTTP Basic; body carries no client_id/secret.
    use base64::Engine;
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("{CID}:a-secret"));
    let body = format!(
        "grant_type=authorization_code&code={code}&redirect_uri={}",
        urlencoding::encode(REDIRECT)
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("authorization", format!("Basic {basic}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert!(json["access_token"].as_str().unwrap().starts_with("forest_oat_"));
}

#[tokio::test]
async fn token_includes_id_token_for_openid_scope() {
    let oauth = Arc::new(MockOAuthAppsClient::new());
    oauth.seed_app(CID, vec![REDIRECT.into()], vec!["openid".into(), "profile".into()]);
    let (state, sessions) = test_state_with_oauth_apps(oauth);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    // Consent with openid scope → code.
    let binding = crate::routes::oauth::consent_binding("test-csrf", CID, REDIRECT, "openid profile");
    let body = format!(
        "_csrf=test-csrf&action=approve&client_id={CID}&redirect_uri={}&scope=openid+profile&consent_binding={binding}",
        urlencoding::encode(REDIRECT)
    );
    let resp = app
        .clone()
        .oneshot(post("/oauth/authorize", &cookie, body))
        .await
        .unwrap();
    let code = query_param(&location(&resp), "code").expect("code");

    let token_body = format!(
        "grant_type=authorization_code&client_id={CID}&client_secret=s&code={code}&redirect_uri={}",
        urlencoding::encode(REDIRECT)
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(token_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert!(json.get("id_token").is_some(), "openid scope → id_token present");
}

#[tokio::test]
async fn discovery_document_advertises_endpoints_and_oidc() {
    let (state, _sessions) = test_state_with_oauth_apps(seeded());
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/openid-configuration")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let doc: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert!(doc["authorization_endpoint"].as_str().unwrap().ends_with("/oauth/authorize"));
    assert!(doc["token_endpoint"].as_str().unwrap().ends_with("/oauth/token"));
    assert!(doc["userinfo_endpoint"].as_str().unwrap().ends_with("/oauth/userinfo"));
    assert_eq!(doc["id_token_signing_alg_values_supported"][0], "HS256");
    let scopes = doc["scopes_supported"].as_array().unwrap();
    assert!(scopes.iter().any(|s| s == "openid"));
}

#[tokio::test]
async fn token_unsupported_grant_type_is_400() {
    let (state, _sessions) = test_state_with_oauth_apps(seeded());
    let app = build_router(state);

    // `password` rather than `client_credentials`: the latter used to be
    // the example of an unsupported grant here, and became supported.
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("grant_type=password"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(json["error"], "unsupported_grant_type");
}

#[tokio::test]
async fn token_client_credentials_returns_a_bearer_token() {
    let (state, _sessions) = test_state_with_oauth_apps(seeded());
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "grant_type=client_credentials&client_id=cid&client_secret=secret&scope=profile",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(json["token_type"], "bearer");
    assert!(json["access_token"].as_str().is_some_and(|t| !t.is_empty()));
    assert_eq!(json["scope"], "profile");
    // A machine token carries no user, so there is nothing to refresh
    // and no subject to describe.
    assert!(json.get("refresh_token").is_none());
    assert!(json.get("id_token").is_none());
}

#[tokio::test]
async fn userinfo_returns_claims_with_valid_bearer() {
    let (state, _sessions) = test_state_with_oauth_apps(seeded());
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/oauth/userinfo")
                .header("authorization", "Bearer forest_oat_mocktoken")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(json["sub"], "user-1");
    assert_eq!(json["username"], "testuser");
    assert_eq!(json["email"], "test@example.com");
}

#[tokio::test]
async fn userinfo_without_bearer_is_401() {
    let (state, _sessions) = test_state_with_oauth_apps(seeded());
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/oauth/userinfo")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(resp.headers().get("www-authenticate").is_some());
}

#[tokio::test]
async fn userinfo_with_bad_token_is_401() {
    let (state, _sessions) = test_state_with_oauth_apps(seeded());
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/oauth/userinfo")
                .header("authorization", "Bearer not-a-real-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn authorized_apps_page_lists_seeded_app() {
    let (state, sessions) = test_state_with_oauth_apps(seeded());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/settings/authorized-apps")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    assert!(text.contains("Authorized applications"));
    assert!(text.contains("Seeded App"));
}

#[tokio::test]
async fn authorized_apps_revoke_redirects_and_requires_csrf() {
    let (state, sessions) = test_state_with_oauth_apps(seeded());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    // Bad CSRF → 403.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/authorized-apps/app-1/revoke")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=wrong"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Valid CSRF → redirect back to the list.
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/authorized-apps/app-1/revoke")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_redirection());
    assert_eq!(
        resp.headers().get("location").unwrap().to_str().unwrap(),
        "/settings/authorized-apps"
    );
}

#[tokio::test]
async fn token_invalid_code_is_invalid_grant() {
    let (state, _sessions) = test_state_with_oauth_apps(seeded());
    let app = build_router(state);

    let body = format!(
        "grant_type=authorization_code&client_id={CID}&client_secret=s&code=nope&redirect_uri={}",
        urlencoding::encode(REDIRECT)
    );
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json: serde_json::Value = serde_json::from_str(&body_text(resp).await).unwrap();
    assert_eq!(json["error"], "invalid_grant");
}
