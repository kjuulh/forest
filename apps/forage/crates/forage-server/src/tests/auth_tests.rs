use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::auth::*;
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

// ─── Signup ─────────────────────────────────────────────────────────

#[tokio::test]
async fn signup_page_returns_200() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/signup")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn signup_page_contains_form() {
    let response = test_app()
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
    assert!(html.contains("username"));
    assert!(html.contains("email"));
    assert!(html.contains("password"));
}

#[tokio::test]
async fn signup_duplicate_shows_error() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        register_result: Some(Err(AuthError::AlreadyExists("username taken".into()))),
        ..Default::default()
    });
    let response = test_app_with(mock)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/signup")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "username=testuser&email=test@example.com&password=SecurePass123&password_confirm=SecurePass123",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("already registered"));
}

#[tokio::test]
async fn signup_when_forest_unavailable_shows_error() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        register_result: Some(Err(AuthError::Unavailable("connection refused".into()))),
        ..Default::default()
    });
    let response = test_app_with(mock)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/signup")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "username=testuser&email=test@example.com&password=SecurePass123&password_confirm=SecurePass123",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("temporarily unavailable"));
}

#[tokio::test]
async fn signup_password_too_short_shows_validation_error() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/signup")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "username=testuser&email=test@example.com&password=short&password_confirm=short",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("at least 12"));
}

#[tokio::test]
async fn signup_password_mismatch_shows_error() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/signup")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "username=testuser&email=test@example.com&password=SecurePass123&password_confirm=differentpassword",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("do not match"));
}

// ─── Login ──────────────────────────────────────────────────────────

#[tokio::test]
async fn login_page_returns_200() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn login_page_contains_form() {
    let response = test_app()
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
    assert!(html.contains("identifier"));
    assert!(html.contains("password"));
}

#[tokio::test]
async fn login_submit_success_sets_session_cookie() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "identifier=testuser&password=CorrectPass123",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get("location").unwrap(), "/dashboard");
    // Should have a single forage_session cookie
    let cookies: Vec<_> = response.headers().get_all("set-cookie").iter().collect();
    assert!(!cookies.is_empty());
    let cookie_str = cookies[0].to_str().unwrap();
    assert!(cookie_str.contains("forage_session="));
    assert!(cookie_str.contains("HttpOnly"));
}

#[tokio::test]
async fn login_submit_bad_credentials_shows_error() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("identifier=testuser&password=wrongpassword"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Invalid"));
}

#[tokio::test]
async fn login_when_forest_unavailable_shows_error() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        login_result: Some(Err(AuthError::Unavailable("connection refused".into()))),
        ..Default::default()
    });
    let response = test_app_with(mock)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("identifier=testuser&password=CorrectPass123"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("temporarily unavailable"));
}

#[tokio::test]
async fn login_empty_fields_shows_validation_error() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("identifier=&password="))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("required"));
}

// ─── Session / Dashboard ────────────────────────────────────────────

#[tokio::test]
async fn dashboard_without_auth_redirects_to_login() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get("location").unwrap(), "/login");
}

#[tokio::test]
async fn dashboard_with_session_shows_page() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Dashboard now renders a proper page
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn dashboard_with_expired_token_refreshes_transparently() {
    let (state, sessions) = test_state();
    let cookie = create_expired_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Should succeed (render dashboard) because refresh_token works
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn dashboard_with_invalid_session_redirects() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", "forage_session=nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get("location").unwrap(), "/login");
}

#[tokio::test]
async fn old_token_cookies_are_ignored() {
    // Old-style cookies should not authenticate
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", "forage_access=mock-access; forage_refresh=mock-refresh")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get("location").unwrap(), "/login");
}

#[tokio::test]
async fn expired_session_with_failed_refresh_redirects_to_login() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        refresh_result: Some(Err(AuthError::NotAuthenticated)),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(mock, MockPlatformClient::new());
    let cookie = create_expired_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get("location").unwrap(), "/login");

    // Session should be destroyed
    assert_eq!(sessions.session_count(), 0);
}

#[tokio::test]
async fn login_with_remember_me_sets_persistent_cookie() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "identifier=testuser&password=CorrectPass123&remember_me=on",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookie_str = response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(cookie_str.contains("forage_session="));
    assert!(cookie_str.contains("Max-Age="));
}

#[tokio::test]
async fn login_without_remember_me_sets_session_cookie() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "identifier=testuser&password=CorrectPass123",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookie_str = response
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(cookie_str.contains("forage_session="));
    assert!(!cookie_str.contains("Max-Age="));
}

// ─── Logout ─────────────────────────────────────────────────────────

#[tokio::test]
async fn logout_destroys_session_and_redirects() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    assert_eq!(sessions.session_count(), 1);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers().get("location").unwrap(), "/");

    // Session should be destroyed
    assert_eq!(sessions.session_count(), 0);
}

#[tokio::test]
async fn logout_with_invalid_csrf_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/logout")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=wrong-token"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Session should NOT be destroyed
    assert_eq!(sessions.session_count(), 1);
}

// ─── return_to redirect contract (DATA-251) ──────────────────────────

#[tokio::test]
async fn signup_page_with_return_to_renders_hidden_field() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/signup?return_to=%2Fdevice%3Fuser_code%3DABCD-EFGH")
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
    assert!(
        html.contains(r#"<input type="hidden" name="return_to""#),
        "signup form must carry return_to as a hidden field"
    );
    assert!(
        html.contains("ABCD-EFGH"),
        "user_code must be preserved in the rendered form"
    );
}

#[tokio::test]
async fn signup_submit_honours_return_to() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/signup")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "username=testuser&email=test@example.com&password=SecurePass123\
                     &password_confirm=SecurePass123\
                     &return_to=%2Fdevice%3Fuser_code%3DABCD-EFGH",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "successful signup must redirect (303), got {:?}",
        response.status()
    );
    let location = response
        .headers()
        .get("location")
        .expect("location header")
        .to_str()
        .unwrap();
    assert_eq!(location, "/device?user_code=ABCD-EFGH");
}

#[tokio::test]
async fn signup_submit_rejects_external_return_to_falls_back_to_dashboard() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/signup")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "username=testuser&email=test@example.com&password=SecurePass123\
                     &password_confirm=SecurePass123\
                     &return_to=https%3A%2F%2Fevil.com%2Fphish",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get("location")
        .expect("location header")
        .to_str()
        .unwrap();
    assert_eq!(
        location, "/dashboard",
        "external URLs must be ignored, not followed"
    );
}

#[tokio::test]
async fn login_page_create_one_link_carries_return_to() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/login?return_to=%2Fdevice%3Fuser_code%3DABCD-EFGH")
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
    assert!(
        html.contains(r#"href="/signup?return_to="#),
        "login template's /signup link must carry return_to"
    );
}

/// XSS guard: a return_to that survives `safe_return_to` (must start
/// with `/`) but contains HTML-special characters must be escaped when
/// rendered into the signup form's hidden field. Without MiniJinja
/// autoescape on this would be a stored XSS — `/foo"><script>...` would
/// break out of the value attribute.
#[tokio::test]
async fn signup_form_escapes_hostile_return_to() {
    let raw = r#"/foo"><script>alert(1)</script><input value=""#;
    let encoded = urlencoding::encode(raw).into_owned();
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri(&format!("/signup?return_to={encoded}"))
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
    assert!(
        !html.contains("<script>alert(1)</script>"),
        "return_to was not escaped — XSS possible in the hidden field"
    );
}

/// POST /login/mfa with the challenge cookie + a return_to hidden field
/// must land the user on the carried path, not /dashboard. Covers the
/// MFA leg of the device-login flow.
#[tokio::test]
async fn login_mfa_submit_honours_return_to() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login/mfa")
                .header("cookie", "forage_mfa_session=test-mfa-token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "code=123456&return_to=%2Fdevice%3Fuser_code%3DMFAQ-2025",
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
        location, "/device?user_code=MFAQ-2025",
        "MFA submit must honour return_to from the hidden form field"
    );
}


