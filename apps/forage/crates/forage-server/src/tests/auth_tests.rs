use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::auth::*;
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

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
        register_result: Some(Err(AuthError::AlreadyExists(AUTH_BACKEND_DETAIL.into()))),
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
    assert!(!html.contains("private_auth"));
    assert!(!html.contains("secret-auth-token"));
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
    assert_generic_auth_error(&html);
    assert!(!html.contains("connection refused"));
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
    assert_generic_auth_error(&html);
    assert!(!html.contains("connection refused"));
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

    assert_eq!(sessions.session_count(), 1);
}

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


const AUTH_BACKEND_DETAIL: &str =
    "SELECT password_hash FROM private_auth.users; backend.internal:5432; secret-auth-token";

fn assert_generic_auth_error(html: &str) -> uuid::Uuid {
    assert!(!html.contains("SELECT password_hash"));
    assert!(!html.contains("private_auth"));
    assert!(!html.contains("backend.internal"));
    assert!(!html.contains("secret-auth-token"));
    html.split(|c: char| !c.is_ascii_hexdigit() && c != '-')
        .filter_map(|value| uuid::Uuid::parse_str(value).ok())
        .find(|value| value.get_version_num() == 4)
        .expect("unexpected auth errors must carry a server-minted UUID reference")
}

#[tokio::test]
async fn signup_backend_failure_is_generic_and_does_not_create_a_session() {
    for error in [
        AuthError::Other(AUTH_BACKEND_DETAIL.into()),
        AuthError::Unavailable(AUTH_BACKEND_DETAIL.into()),
        AuthError::PermissionDenied(AUTH_BACKEND_DETAIL.into()),
    ] {
        let mock = MockForestClient::with_behavior(MockBehavior {
            register_result: Some(Err(error)),
            ..Default::default()
        });
        let (state, sessions) = test_state_with(mock, MockPlatformClient::new());
        let response = build_router(state)
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
        assert!(response.headers().get("set-cookie").is_none());
        assert_eq!(sessions.session_count(), 0);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_generic_auth_error(std::str::from_utf8(&body).unwrap());
    }
}

#[tokio::test]
async fn login_backend_failure_is_generic_and_does_not_create_a_session() {
    let mut references = std::collections::HashSet::new();
    for error in [
        AuthError::Other(AUTH_BACKEND_DETAIL.into()),
        AuthError::Unavailable(AUTH_BACKEND_DETAIL.into()),
        AuthError::PermissionDenied(AUTH_BACKEND_DETAIL.into()),
    ] {
        let mock = MockForestClient::with_behavior(MockBehavior {
            login_result: Some(Err(error)),
            ..Default::default()
        });
        let (state, sessions) = test_state_with(mock, MockPlatformClient::new());
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("x-request-id", "caller-selected-reference")
                    .body(Body::from("identifier=testuser&password=CorrectPass123"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("set-cookie").is_none());
        assert_eq!(sessions.session_count(), 0);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = std::str::from_utf8(&body).unwrap();
        assert!(!html.contains("caller-selected-reference"));
        assert!(references.insert(assert_generic_auth_error(html)));
    }
}

#[tokio::test]
async fn mfa_backend_failure_is_generic_and_keeps_the_challenge_unauthenticated() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        verify_login_mfa_result: Some(Err(AuthError::Other(AUTH_BACKEND_DETAIL.into()))),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(mock, MockPlatformClient::new());
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login/mfa")
                .header("cookie", "forage_mfa_session=test-mfa-token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("code=123456&return_to=%2Fdevice"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("set-cookie").is_none());
    assert_eq!(sessions.session_count(), 0);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = std::str::from_utf8(&body).unwrap();
    assert_generic_auth_error(html);
}

#[tokio::test]
async fn mfa_settings_backend_failures_are_generic_without_changing_status() {
    for (uri, form, behavior) in [
        (
            "/settings/account/mfa/verify",
            "_csrf=test-csrf&mfa_id=test-mfa&code=123456",
            MockBehavior {
                verify_mfa_setup_result: Some(Err(AuthError::Other(AUTH_BACKEND_DETAIL.into()))),
                ..Default::default()
            },
        ),
        (
            "/settings/account/mfa/disable",
            "_csrf=test-csrf&code=123456",
            MockBehavior {
                disable_mfa_result: Some(Err(AuthError::Other(AUTH_BACKEND_DETAIL.into()))),
                ..Default::default()
            },
        ),
    ] {
        let (state, sessions) = test_state_with(
            MockForestClient::with_behavior(behavior),
            MockPlatformClient::new(),
        );
        let cookie = create_test_session(&sessions).await;
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("cookie", cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(form))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(sessions.session_count(), 1);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_generic_auth_error(std::str::from_utf8(&body).unwrap());
    }
}

#[tokio::test]
async fn account_backend_failures_keep_forms_without_exposing_diagnostics() {
    for (uri, form, behavior) in [
        (
            "/settings/account/username",
            "_csrf=test-csrf&username=newuser",
            MockBehavior {
                update_username_result: Some(Err(AuthError::Other(AUTH_BACKEND_DETAIL.into()))),
                ..Default::default()
            },
        ),
        (
            "/settings/account/password",
            "_csrf=test-csrf&current_password=CorrectPass123&new_password=SecurePass123&new_password_confirm=SecurePass123",
            MockBehavior {
                change_password_result: Some(Err(AuthError::Other(AUTH_BACKEND_DETAIL.into()))),
                ..Default::default()
            },
        ),
        (
            "/settings/account/emails",
            "_csrf=test-csrf&email=new@example.com",
            MockBehavior {
                add_email_result: Some(Err(AuthError::Other(AUTH_BACKEND_DETAIL.into()))),
                ..Default::default()
            },
        ),
        (
            "/settings/account/emails/remove",
            "_csrf=test-csrf&email=test@example.com",
            MockBehavior {
                remove_email_result: Some(Err(AuthError::Other(AUTH_BACKEND_DETAIL.into()))),
                ..Default::default()
            },
        ),
    ] {
        let (state, sessions) = test_state_with(
            MockForestClient::with_behavior(behavior),
            MockPlatformClient::new(),
        );
        let cookie = create_test_session(&sessions).await;
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("cookie", cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(form))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(sessions.session_count(), 1);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_generic_auth_error(std::str::from_utf8(&body).unwrap());
    }
}
