use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::auth::magic_link::{generate_magic_link_token, InMemoryMagicLinkStore, MagicLinkStore};
use forage_core::auth::*;
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

// ─── Request magic link ─────────────────────────────────────────────

#[tokio::test]
async fn magic_link_request_returns_check_email_page() {
    let (state, _sessions) = test_state_with_magic_link();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/magic-link")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("email=test@example.com"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Check your email"));
    assert!(html.contains("test@example.com"));
}

#[tokio::test]
async fn magic_link_request_invalid_email_shows_error() {
    let (state, _sessions) = test_state_with_magic_link();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/magic-link")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("email=notanemail"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    // Should re-render the login page with an error
    assert!(html.contains("Sign in"));
}

#[tokio::test]
async fn magic_link_returns_503_when_not_configured() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/magic-link")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("email=test@example.com"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ─── Verify magic link ──────────────────────────────────────────────

#[tokio::test]
async fn magic_link_verify_valid_token_redirects_to_dashboard() {
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let (raw, hash) = generate_magic_link_token();
    let expires = chrono::Utc::now() + chrono::Duration::minutes(15);
    store
        .store_token(forage_core::auth::magic_link::TOKEN_TYPE_MAGIC_LINK, &hash, "test@example.com", expires)
        .await
        .unwrap();

    let (state, _sessions) = test_state();
    let state = state.with_magic_link_store(store);
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri(&format!("/auth/magic-link/verify?token={}", urlencoding::encode(&raw)))
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
}

#[tokio::test]
async fn magic_link_verify_new_user_redirects_to_complete_profile() {
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let (raw, hash) = generate_magic_link_token();
    let expires = chrono::Utc::now() + chrono::Duration::minutes(15);
    store
        .store_token(forage_core::auth::magic_link::TOKEN_TYPE_MAGIC_LINK, &hash, "test@example.com", expires)
        .await
        .unwrap();

    let mock = MockForestClient::with_behavior(MockBehavior {
        oauth_login_result: Some(Ok(OAuthLoginResult {
            user: ok_user(),
            tokens: ok_tokens(),
            is_new_user: true,
        })),
        ..Default::default()
    });
    let (state, _sessions) = test_state_with(mock, MockPlatformClient::new());
    let state = state.with_magic_link_store(store);
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri(&format!("/auth/magic-link/verify?token={}", urlencoding::encode(&raw)))
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
async fn magic_link_verify_expired_token_shows_error() {
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let (raw, hash) = generate_magic_link_token();
    // Already expired
    let expired = chrono::Utc::now() - chrono::Duration::seconds(1);
    store
        .store_token(forage_core::auth::magic_link::TOKEN_TYPE_MAGIC_LINK, &hash, "test@example.com", expired)
        .await
        .unwrap();

    let (state, _sessions) = test_state();
    let state = state.with_magic_link_store(store);
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri(&format!("/auth/magic-link/verify?token={}", urlencoding::encode(&raw)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn magic_link_verify_consumed_token_fails_on_reuse() {
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let (raw, hash) = generate_magic_link_token();
    let expires = chrono::Utc::now() + chrono::Duration::minutes(15);
    store
        .store_token(forage_core::auth::magic_link::TOKEN_TYPE_MAGIC_LINK, &hash, "test@example.com", expires)
        .await
        .unwrap();

    // Consume the token first
    store
        .verify_and_consume(forage_core::auth::magic_link::TOKEN_TYPE_MAGIC_LINK, &hash)
        .await
        .unwrap();

    let (state, _sessions) = test_state();
    let state = state.with_magic_link_store(store);
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri(&format!("/auth/magic-link/verify?token={}", urlencoding::encode(&raw)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn magic_link_verify_missing_token_returns_400() {
    let (state, _sessions) = test_state_with_magic_link();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/magic-link/verify")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

// ─── Template visibility ────────────────────────────────────────────

#[tokio::test]
async fn login_page_shows_magic_link_when_configured() {
    let (state, _sessions) = test_state_with_magic_link();
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
    assert!(html.contains("Sign in with email link"));
    assert!(html.contains("/auth/magic-link"));
}

#[tokio::test]
async fn login_page_hides_magic_link_when_not_configured() {
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
    assert!(!html.contains("Sign in with email link"));
}
