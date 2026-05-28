use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::auth::*;
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

// ─── Google OAuth Start ─────────────────────────────────────────────

#[tokio::test]
async fn google_oauth_start_redirects_to_google() {
    let (state, _sessions) = test_state_with_google_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FOUND);
    let location = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(location.contains("accounts.google.com"));
    assert!(location.contains("client_id=test-google-client-id"));
    assert!(location.contains("redirect_uri="));
    assert!(location.contains("response_type=code"));
    assert!(location.contains("state="));
}

#[tokio::test]
async fn google_oauth_start_sets_state_cookie() {
    let (state, _sessions) = test_state_with_google_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let cookies: Vec<_> = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    let has_state_cookie = cookies
        .iter()
        .any(|c| c.starts_with("forage_oauth_state=") && c.contains("HttpOnly"));
    assert!(has_state_cookie, "expected forage_oauth_state cookie, got: {cookies:?}");
}

#[tokio::test]
async fn google_oauth_start_returns_503_when_not_configured() {
    // Default test_state has no google_oauth_config
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ─── Google OAuth Callback ──────────────────────────────────────────

#[tokio::test]
async fn google_callback_existing_user_redirects_to_dashboard() {
    let (state, _sessions) = test_state_with_google_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google/callback?code=test-code&state=test-state")
                .header("cookie", "forage_oauth_state=test-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(location, "/dashboard");

    // Should set a session cookie
    let cookies: Vec<_> = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert!(
        cookies.iter().any(|c| c.starts_with("forage_session=")),
        "expected session cookie, got: {cookies:?}"
    );
}

#[tokio::test]
async fn google_callback_new_user_redirects_to_complete_profile() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        oauth_login_result: Some(Ok(OAuthLoginResult {
            user: ok_user(),
            tokens: ok_tokens(),
            is_new_user: true,
        })),
        ..Default::default()
    });
    let (state, _sessions) = test_state_with(mock, MockPlatformClient::new());
    let state = state
        .with_google_oauth_config(crate::state::GoogleOAuthConfig {
            client_id: "test-google-client-id".into(),
            client_secret: "test-google-client-secret".into(),
            redirect_host: "http://localhost:3000".into(),
        })
        .with_google_oidc_exchange(std::sync::Arc::new(MockOidcExchange::new()));
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google/callback?code=test-code&state=test-state")
                .header("cookie", "forage_oauth_state=test-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(location, "/auth/complete-profile");
}

#[tokio::test]
async fn google_callback_state_mismatch_returns_403() {
    let (state, _sessions) = test_state_with_google_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google/callback?code=test-code&state=wrong-state")
                .header("cookie", "forage_oauth_state=correct-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn google_callback_missing_state_cookie_returns_403() {
    let (state, _sessions) = test_state_with_google_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google/callback?code=test-code&state=some-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn google_callback_with_error_redirects_to_login() {
    // OAuth 2.0 RFC 6749 §4.1.2.1 requires the provider to return the
    // original `state` alongside any error. We now validate state
    // before reading other query params (adversarial review #3), so
    // this test sends the state cookie + matching state param to
    // exercise the realistic error path.
    let (state, _sessions) = test_state_with_google_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google/callback?error=access_denied&state=test-state")
                .header("cookie", "forage_oauth_state=test-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(location, "/login");
}

#[tokio::test]
async fn google_callback_with_error_and_no_state_is_rejected() {
    // Spec-violating providers (or attackers) sending an error without
    // the state parameter should fail at CSRF validation. Closes the
    // info-leak path adversarial review #3 documented.
    let (state, _sessions) = test_state_with_google_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google/callback?error=access_denied")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn google_callback_with_empty_state_cookie_returns_403() {
    // Defense in depth: an injected empty-value state cookie combined
    // with a missing `?state=` query parameter (both empty strings)
    // must not bypass CSRF. Closes adversarial review pass 2 MEDIUM-1.
    let (state, _sessions) = test_state_with_google_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google/callback?code=x&state=")
                .header("cookie", "forage_oauth_state=")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn google_callback_forest_unavailable_shows_error() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        oauth_login_result: Some(Err(AuthError::Unavailable("connection refused".into()))),
        ..Default::default()
    });
    let (state, _sessions) = test_state_with(mock, MockPlatformClient::new());
    let state = state
        .with_google_oauth_config(crate::state::GoogleOAuthConfig {
            client_id: "test-google-client-id".into(),
            client_secret: "test-google-client-secret".into(),
            redirect_host: "http://localhost:3000".into(),
        })
        .with_google_oidc_exchange(std::sync::Arc::new(MockOidcExchange::new()));
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google/callback?code=test-code&state=test-state")
                .header("cookie", "forage_oauth_state=test-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ─── Complete Profile ───────────────────────────────────────────────

#[tokio::test]
async fn complete_profile_renders_for_new_user() {
    let (state, sessions) = test_state_with_google_oauth();
    let cookie = create_test_session_needs_username(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/complete-profile")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Choose your username"));
    assert!(html.contains("username"));
}

#[tokio::test]
async fn complete_profile_redirects_when_not_needed() {
    let (state, sessions) = test_state_with_google_oauth();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/complete-profile")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn complete_profile_submit_updates_username() {
    let (state, sessions) = test_state_with_google_oauth();
    let cookie = create_test_session_needs_username(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/complete-profile")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf&username=alice"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(location, "/dashboard");
}

#[tokio::test]
async fn complete_profile_submit_invalid_username_shows_error() {
    let (state, sessions) = test_state_with_google_oauth();
    let cookie = create_test_session_needs_username(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/complete-profile")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf&username=ab"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Choose your username"));
}

// ─── Template visibility ────────────────────────────────────────────

#[tokio::test]
async fn login_page_shows_google_button_when_configured() {
    let (state, _sessions) = test_state_with_google_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Continue with Google"));
    assert!(html.contains("/auth/google"));
}

#[tokio::test]
async fn login_page_hides_google_button_when_not_configured() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(!html.contains("Continue with Google"));
}

#[tokio::test]
async fn signup_page_shows_google_button_when_configured() {
    let (state, _sessions) = test_state_with_google_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/signup")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Continue with Google"));
}

// ─── GitHub callback parity tests ────────────────────────────────────
//
// The GitHub callback shares its structure with Google's. The
// link-flow path is covered in account_link_tests.rs; these tests
// cover the bare login error paths and CSRF edge cases that adversarial
// review pass 2 NIT-3 flagged as Google-only.

#[tokio::test]
async fn github_callback_with_error_and_valid_state_redirects_to_login() {
    let (state, _sessions) = test_state_with_github_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/github/callback?error=access_denied&state=test-state")
                .header("cookie", "forage_oauth_state=test-state")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(location, "/login");
}

#[tokio::test]
async fn github_callback_with_error_and_no_state_is_rejected() {
    let (state, _sessions) = test_state_with_github_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/github/callback?error=access_denied")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn github_callback_with_empty_state_cookie_returns_403() {
    let (state, _sessions) = test_state_with_github_oauth();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/github/callback?code=x&state=")
                .header("cookie", "forage_oauth_state=")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ─── return_to round-trip through the OAuth state store (DATA-251) ──

/// Drive the full Google sign-in flow end-to-end:
///   start with `?return_to=/device?user_code=…` → persists state row →
///   callback consumes the row → final redirect points at the original
///   intent, not `/dashboard`.
#[tokio::test]
async fn google_oauth_flow_round_trips_return_to() {
    let (state, _sessions) = test_state_with_google_oauth();
    let app = build_router(state);

    // 1. Start the flow with a return_to. We can't observe the random
    //    state token directly, so we pull it back from the Set-Cookie
    //    header (the cookie value IS the state token).
    let start_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/google?return_to=%2Fdevice%3Fuser_code%3DABCD-EFGH")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(start_response.status(), StatusCode::FOUND);
    let state_cookie = start_response
        .headers()
        .get_all("set-cookie")
        .iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            s.strip_prefix("forage_oauth_state=")
                .and_then(|rest| rest.split(';').next())
        })
        .expect("forage_oauth_state cookie")
        .to_string();

    // 2. Hit the callback with the same state — same browser would replay
    //    the cookie. The state-store row is keyed by the state token, so
    //    the return_to we stored at step 1 should now drive the redirect.
    let callback_response = app
        .oneshot(
            Request::builder()
                .uri(&format!(
                    "/auth/google/callback?code=test-code&state={}",
                    state_cookie
                ))
                .header(
                    "cookie",
                    format!("forage_oauth_state={state_cookie}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(callback_response.status(), StatusCode::SEE_OTHER);
    let location = callback_response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(
        location, "/device?user_code=ABCD-EFGH",
        "OAuth flow must honour the return_to stored at start"
    );
}

/// New-OAuth-user case: the redirect must forward return_to through the
/// /auth/complete-profile step so the device approval still happens
/// after the user picks a username.
#[tokio::test]
async fn google_oauth_new_user_forwards_return_to_through_complete_profile() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        oauth_login_result: Some(Ok(OAuthLoginResult {
            user: ok_user(),
            tokens: ok_tokens(),
            is_new_user: true,
        })),
        ..Default::default()
    });
    let (state, _sessions) = test_state_with(mock, MockPlatformClient::new());
    let state = state
        .with_google_oauth_config(crate::state::GoogleOAuthConfig {
            client_id: "test-google-client-id".into(),
            client_secret: "test-google-client-secret".into(),
            redirect_host: "http://localhost:3000".into(),
        })
        .with_google_oidc_exchange(std::sync::Arc::new(MockOidcExchange::new()));
    let app = build_router(state);

    let start_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/auth/google?return_to=%2Fdevice%3Fuser_code%3DZZZZ-YYYY")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let state_cookie = start_response
        .headers()
        .get_all("set-cookie")
        .iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            s.strip_prefix("forage_oauth_state=")
                .and_then(|rest| rest.split(';').next())
        })
        .expect("forage_oauth_state cookie")
        .to_string();

    let response = app
        .oneshot(
            Request::builder()
                .uri(&format!(
                    "/auth/google/callback?code=test-code&state={state_cookie}"
                ))
                .header("cookie", format!("forage_oauth_state={state_cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        location.starts_with("/auth/complete-profile?return_to="),
        "new OAuth user must land on complete-profile carrying return_to, got: {location}"
    );
    assert!(
        location.contains("ZZZZ-YYYY"),
        "user_code must survive into the complete-profile query: {location}"
    );
}

/// After a new OAuth user picks a username, the complete-profile submit
/// must redirect to the carried-through `return_to`, not /dashboard.
/// Closes the second half of the new-user device-login chain.
#[tokio::test]
async fn complete_profile_submit_honours_return_to() {
    let (state, sessions) = test_state_with_google_oauth();
    let cookie = create_test_session_needs_username(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/complete-profile")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "_csrf=test-csrf&username=alice\
                     &return_to=%2Fdevice%3Fuser_code%3DABCD-EFGH",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(
        location, "/device?user_code=ABCD-EFGH",
        "complete-profile submit must honour return_to from the form"
    );
}
