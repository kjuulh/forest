//! Integration tests for the OAuth account-linking routes added in
//! spec 010-account-integrations. These exercise:
//!
//! - GET  /settings/account/{github,google}/connect
//! - POST /settings/account/{github,google}/disconnect
//! - Dispatching the existing /auth/{provider}/callback into the link
//!   flow when the `forage_oauth_link_user` cookie is present.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::auth::*;
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

// ─── Helpers ─────────────────────────────────────────────────────────

fn url_decode(s: &str) -> String {
    urlencoding::decode(s).map(|c| c.into_owned()).unwrap_or_default()
}

/// Extract the value of a Set-Cookie header matching the given cookie name.
fn extract_cookie(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with(&format!("{name}=")))
        .map(|c| {
            let after_eq = &c[name.len() + 1..];
            after_eq
                .split(';')
                .next()
                .unwrap_or("")
                .to_string()
        })
}

// ─── /settings/account/github/connect ────────────────────────────────

#[tokio::test]
async fn github_link_start_unauthenticated_redirects_to_login() {
    let (state, _sessions) = test_state_with_github_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/settings/account/github/connect")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // No session: middleware redirects to /login (302).
    assert!(response.status().is_redirection(), "expected redirect, got {}", response.status());
    let location = response.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        location.contains("/login"),
        "unauthenticated user should be sent to login, got: {location}"
    );
}

#[tokio::test]
async fn github_link_start_authenticated_redirects_to_github_with_state_and_link_cookies() {
    let (state, sessions) = test_state_with_github_oauth();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/settings/account/github/connect")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(response.status().is_redirection(), "expected redirect, got {}", response.status());
    let location = response.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        location.starts_with("https://github.com/login/oauth/authorize"),
        "should redirect to github authorize URL, got: {location}"
    );
    assert!(location.contains("client_id=test-github-client-id"));
    assert!(location.contains(&url_decode("scope=read%3Auser%20user%3Aemail")) || location.contains("scope=read%3Auser%20user%3Aemail"));

    // Both the state cookie AND the link-purpose cookie must be set.
    let state_cookie = extract_cookie(response.headers(), "forage_oauth_state");
    assert!(state_cookie.is_some(), "missing state cookie");

    let link_cookie = extract_cookie(response.headers(), "forage_oauth_link_user");
    assert_eq!(
        link_cookie.as_deref(),
        Some("user-123"),
        "link cookie should encode session user_id"
    );
}

#[tokio::test]
async fn login_start_clears_stale_link_cookie() {
    // Regression: a user may have started a link flow and abandoned it,
    // leaving forage_oauth_link_user set. A subsequent click on
    // "Sign in with GitHub" must clear it so the callback dispatches
    // into the login branch (not the link branch).
    let (state, _sessions) = test_state_with_github_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/github")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let clear = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("forage_oauth_link_user="));
    assert!(
        clear.is_some(),
        "login-start should emit a Set-Cookie clearing the link-purpose cookie"
    );
    let clear = clear.unwrap();
    assert!(
        clear.contains("Max-Age=0"),
        "expected Max-Age=0 to clear the cookie, got: {clear}"
    );
}

#[tokio::test]
async fn github_link_start_returns_503_when_oauth_not_configured() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/settings/account/github/connect")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ─── /settings/account/google/connect ────────────────────────────────

#[tokio::test]
async fn google_link_start_authenticated_redirects_to_google_with_link_cookie() {
    let (state, sessions) = test_state_with_google_oauth();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/settings/account/google/connect")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(response.status().is_redirection(), "expected redirect, got {}", response.status());
    let location = response.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        location.starts_with("https://accounts.google.com/"),
        "should redirect to google authorize URL, got: {location}"
    );

    let link_cookie = extract_cookie(response.headers(), "forage_oauth_link_user");
    assert_eq!(link_cookie.as_deref(), Some("user-123"));
}

// ─── /auth/github/callback dispatching ───────────────────────────────

#[tokio::test]
async fn github_callback_link_flow_calls_link_oauth_provider() {
    use std::sync::Arc;

    let mock = MockForestClient::new();
    let (state, sessions) = test_state_with(mock, MockPlatformClient::new());
    let state = state
        .with_github_oauth_config(crate::state::GitHubOAuthConfig {
            client_id: "test-github-client-id".into(),
            client_secret: "test-github-client-secret".into(),
            redirect_host: "http://localhost:3000".into(),
        })
        .with_github_oidc_exchange(Arc::new(MockOidcExchange::with_result(Ok(
            OidcIdentity {
                sub: "12345".into(),
                email: "kasper@understory.io".into(),
                name: "Kasper Hermansen".into(),
                picture_url: None,
                login: Some("kjuulh".into()),
            },
        ))));
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/github/callback?code=test-code&state=test-state")
                .header(
                    "cookie",
                    format!(
                        "{cookie}; forage_oauth_state=test-state; forage_oauth_link_user=user-123"
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(response.status().is_redirection(), "expected redirect, got {}", response.status());
    let location = response.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        location.starts_with("/settings/account"),
        "link flow should redirect to /settings/account, got: {location}"
    );
    assert!(
        location.contains("flash=linked_github"),
        "expected success flash, got: {location}"
    );
}

#[tokio::test]
async fn github_callback_link_flow_with_mismatched_user_returns_403() {
    let (state, sessions) = test_state_with_github_oauth();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    // The link cookie names a different user than the session.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/github/callback?code=test-code&state=test-state")
                .header(
                    "cookie",
                    format!(
                        "{cookie}; forage_oauth_state=test-state; forage_oauth_link_user=someone-else"
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn github_callback_link_flow_reports_cross_user_conflict() {
    use std::sync::Arc;

    let behavior = MockBehavior {
        link_oauth_provider_result: Some(Err(AuthError::AlreadyExists(
            "this external account is already linked to another user".into(),
        ))),
        ..Default::default()
    };
    let mock = MockForestClient::with_behavior(behavior);
    let (state, sessions) = test_state_with(mock, MockPlatformClient::new());
    let state = state
        .with_github_oauth_config(crate::state::GitHubOAuthConfig {
            client_id: "test-github-client-id".into(),
            client_secret: "test-github-client-secret".into(),
            redirect_host: "http://localhost:3000".into(),
        })
        .with_github_oidc_exchange(Arc::new(MockOidcExchange::new()));
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/github/callback?code=test-code&state=test-state")
                .header(
                    "cookie",
                    format!(
                        "{cookie}; forage_oauth_state=test-state; forage_oauth_link_user=user-123"
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(response.status().is_redirection(), "expected redirect, got {}", response.status());
    let location = response.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        location.contains("already_linked_other_github"),
        "expected cross-user conflict error, got: {location}"
    );
}

#[tokio::test]
async fn google_callback_link_flow_calls_link_oauth_provider() {
    // Symmetric to the github link-flow test. Closes adversarial review
    // gap #9.
    use std::sync::Arc;

    let mock = MockForestClient::new();
    let (state, sessions) = test_state_with(mock, MockPlatformClient::new());
    let state = state
        .with_google_oauth_config(crate::state::GoogleOAuthConfig {
            client_id: "test-google-client-id".into(),
            client_secret: "test-google-client-secret".into(),
            redirect_host: "http://localhost:3000".into(),
        })
        .with_google_oidc_exchange(Arc::new(MockOidcExchange::with_result(Ok(
            OidcIdentity {
                sub: "g-sub-123".into(),
                email: "kasper@understory.io".into(),
                name: "Kasper Hermansen".into(),
                picture_url: None,
                login: None,
            },
        ))));
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google/callback?code=test-code&state=test-state")
                .header(
                    "cookie",
                    format!(
                        "{cookie}; forage_oauth_state=test-state; forage_oauth_link_user=user-123"
                    ),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(response.status().is_redirection(), "expected redirect, got {}", response.status());
    let location = response.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        location.contains("flash=linked_google"),
        "expected google link success flash, got: {location}"
    );
}

#[tokio::test]
async fn callback_with_link_cookie_but_no_state_cookie_returns_403_not_link_error() {
    // Regression for adversarial review #3: state validation must
    // happen *before* the link-cookie dispatch, so an attacker can't
    // probe link-vs-login by planting only a link cookie. With no
    // state cookie, both flows should reject identically with "OAuth
    // state mismatch" — no leakage of dispatch context.
    let (state, sessions) = test_state_with_github_oauth();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/github/callback?code=x&state=spoofed")
                .header(
                    "cookie",
                    format!("{cookie}; forage_oauth_link_user=user-123"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("OAuth state mismatch"),
        "expected generic state-mismatch error, got: {body_str}"
    );
    // Crucially: must NOT contain "Link mismatch" which would leak that
    // we'd otherwise have taken the link branch.
    assert!(
        !body_str.contains("Link mismatch"),
        "state failure must be checked before link mismatch is computed"
    );
}

#[tokio::test]
async fn authenticated_oauth_start_clears_link_cookie_before_redirect() {
    // Regression for adversarial review #4: a logged-in user hitting
    // the bare /auth/github URL must clear any stale link cookie so a
    // later callback can't mis-dispatch.
    let (state, sessions) = test_state_with_github_oauth();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/github")
                .header(
                    "cookie",
                    format!("{cookie}; forage_oauth_link_user=user-123"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(response.status().is_redirection());
    let clear = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("forage_oauth_link_user="))
        .expect("expected Set-Cookie clearing forage_oauth_link_user");
    assert!(clear.contains("Max-Age=0"), "expected cookie clear, got: {clear}");
}

#[tokio::test]
async fn github_callback_login_flow_unaffected_when_no_link_cookie() {
    // Without the link cookie, an unauthenticated callback should still
    // proceed down the login path and end up at /dashboard or /complete-profile.
    let (state, _sessions) = test_state_with_github_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/github/callback?code=test-code&state=test-state")
                .header("cookie", "forage_oauth_state=test-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should not be 403 (link-mismatch). The login flow continues — exact
    // resulting status depends on the mock, but it should be a redirect.
    assert_ne!(response.status(), StatusCode::FORBIDDEN);
}

// ─── /settings/account/{provider}/disconnect ─────────────────────────

#[tokio::test]
async fn github_disconnect_with_valid_csrf_calls_unlink() {
    let mock = MockForestClient::new();
    let (state, sessions) = test_state_with(mock, MockPlatformClient::new());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/account/github/disconnect")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(response.status().is_redirection(), "expected redirect, got {}", response.status());
    let location = response.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(location, "/settings/account");
}

#[tokio::test]
async fn github_disconnect_with_invalid_csrf_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/account/github/disconnect")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=wrong-token"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn google_disconnect_with_valid_csrf_calls_unlink() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/account/google/disconnect")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(response.status().is_redirection(), "expected redirect, got {}", response.status());
    assert_eq!(
        response.headers().get("location").unwrap().to_str().unwrap(),
        "/settings/account"
    );
}

#[tokio::test]
async fn disconnect_unauthenticated_redirects_to_login() {
    let (state, _sessions) = test_state();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/account/github/disconnect")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(response.status().is_redirection(), "expected redirect, got {}", response.status());
    let location = response.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        location.contains("/login"),
        "unauthenticated disconnect should send to login, got: {location}"
    );
}

// ─── /settings/account rendering ─────────────────────────────────────

#[tokio::test]
async fn account_page_renders_linked_accounts_when_forest_returns_identities() {
    let behavior = MockBehavior {
        list_linked_identities_result: Some(Ok(vec![
            LinkedIdentity {
                provider: LinkedProvider::GitHub,
                external_id: "12345".into(),
                display_name: "kjuulh".into(),
                email: Some("kasper@understory.io".into()),
                avatar_url: None,
                linked_at: Some("2026-05-20T10:00:00Z".into()),
                subtitle: None,
                disconnect_key: None,
            },
            LinkedIdentity {
                provider: LinkedProvider::Google,
                external_id: "g-sub".into(),
                display_name: "Kasper Hermansen".into(),
                email: Some("kasper@understory.io".into()),
                avatar_url: None,
                linked_at: None,
                subtitle: None,
                disconnect_key: None,
            },
        ])),
        ..Default::default()
    };
    let mock = MockForestClient::with_behavior(behavior);
    let (state, sessions) = test_state_with(mock, MockPlatformClient::new());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/settings/account")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("Linked accounts"),
        "expected 'Linked accounts' heading"
    );
    assert!(
        body_str.contains("kjuulh") || body_str.contains("Kasper Hermansen"),
        "expected linked identity display names in body"
    );
}

#[tokio::test]
async fn account_page_hides_link_buttons_for_unconfigured_providers() {
    // Default test_state has no GitHub or Google OAuth configured.
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/settings/account")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        !body_str.contains("href=\"/settings/account/github/connect\""),
        "should not show GitHub link button when not configured"
    );
    assert!(
        !body_str.contains("href=\"/settings/account/google/connect\""),
        "should not show Google link button when not configured"
    );
}

#[tokio::test]
async fn account_page_shows_link_buttons_when_provider_configured_and_no_link() {
    let (state, sessions) = test_state_with_both_oauth();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/settings/account")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("/settings/account/github/connect"),
        "should show GitHub link button"
    );
    assert!(
        body_str.contains("/settings/account/google/connect"),
        "should show Google link button"
    );
}

#[tokio::test]
async fn account_page_hides_link_button_when_provider_already_linked() {
    // Provider is configured, but the user already has a linked identity
    // for it — the "Link GitHub" button should not be shown.
    let behavior = MockBehavior {
        list_linked_identities_result: Some(Ok(vec![LinkedIdentity {
            provider: LinkedProvider::GitHub,
            external_id: "12345".into(),
            display_name: "kjuulh".into(),
            email: Some("kasper@understory.io".into()),
            avatar_url: None,
            linked_at: None,
            subtitle: None,
            disconnect_key: None,
        }])),
        ..Default::default()
    };
    let mock = MockForestClient::with_behavior(behavior);
    let (state, sessions) = test_state_with(mock, MockPlatformClient::new());
    let state = state.with_github_oauth_config(crate::state::GitHubOAuthConfig {
        client_id: "test-github-client-id".into(),
        client_secret: "test-github-client-secret".into(),
        redirect_host: "http://localhost:3000".into(),
    });
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/settings/account")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        !body_str.contains("href=\"/settings/account/github/connect\""),
        "should NOT show GitHub link button when GitHub is already linked"
    );
    // The disconnect form should be shown instead.
    assert!(
        body_str.contains("action=\"/settings/account/github/disconnect\""),
        "should show GitHub disconnect form"
    );
}

#[tokio::test]
async fn account_page_renders_flash_banner_on_success_query() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/settings/account?flash=linked_github")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("GitHub account linked"),
        "expected success flash banner"
    );
}

#[tokio::test]
async fn account_page_renders_error_banner_on_conflict_query() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/settings/account?error=already_linked_other_github")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("already linked to another Forest user"),
        "expected cross-user conflict banner"
    );
}
