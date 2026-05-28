//! Route tests for the email-verification flow (signup verification +
//! /auth/verify-email + /auth/verify-email/resend).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::auth::magic_link::{
    generate_magic_link_token, InMemoryMagicLinkStore, MagicLinkStore, TOKEN_TYPE_EMAIL_VERIFY,
    TOKEN_TYPE_MAGIC_LINK,
};
use forage_core::auth::*;
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

#[tokio::test]
async fn signup_with_verification_required_renders_check_inbox_page() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        register_result: Some(Ok(RegisterResult::VerificationRequired)),
        ..Default::default()
    });
    let (state, _sessions) = test_state_with(mock, MockPlatformClient::new());
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let state = state.with_magic_link_store(store.clone());
    let app = build_router(state);

    let body = "username=test-user&email=test@example.com&password=ValidPassword123&password_confirm=ValidPassword123";
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/signup")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Verify your email"));
    assert!(html.contains("test@example.com"));

    // The token must have been stored under the email-verify type.
    let count = store
        .count_recent(
            TOKEN_TYPE_EMAIL_VERIFY,
            "test@example.com",
            chrono::Utc::now() - chrono::Duration::minutes(15),
        )
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn login_with_email_not_verified_renders_check_inbox_and_enqueues() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        login_result: Some(Ok(LoginResult::EmailNotVerified)),
        ..Default::default()
    });
    let (state, _sessions) = test_state_with(mock, MockPlatformClient::new());
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let state = state.with_magic_link_store(store.clone());
    let app = build_router(state);

    let body = "identifier=test@example.com&password=AnyPassword";
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Verify your email"));

    // A verification email must have been enqueued.
    let count = store
        .count_recent(
            TOKEN_TYPE_EMAIL_VERIFY,
            "test@example.com",
            chrono::Utc::now() - chrono::Duration::minutes(15),
        )
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn verify_email_redeem_valid_token_calls_forest_and_renders_success() {
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let (raw, hash) = generate_magic_link_token();
    let expires = chrono::Utc::now() + chrono::Duration::minutes(15);
    store
        .store_token(TOKEN_TYPE_EMAIL_VERIFY, &hash, "kasper@understory.io", expires, None)
        .await
        .unwrap();

    let (state, _sessions) = test_state();
    let state = state.with_magic_link_store(store.clone());
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri(&format!("/auth/verify-email?token={}", urlencoding::encode(&raw)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    // The Referer of any link click on this page must not leak the
    // redeemed token.
    let referrer_policy = response
        .headers()
        .get("referrer-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(referrer_policy, "no-referrer");

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Email verified"));
    assert!(html.contains("kasper@understory.io"));

    // Single-use absolute: a second click on the same token returns the
    // failure page even though the email is now verified upstream.
    let response2 = build_router({
        let (state, _sessions) = test_state();
        state.with_magic_link_store(store.clone())
    })
    .oneshot(
        Request::builder()
            .uri(&format!("/auth/verify-email?token={}", urlencoding::encode(&raw)))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response2.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn verify_email_redeem_expired_token_renders_failed() {
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let (raw, hash) = generate_magic_link_token();
    store
        .store_token(
            TOKEN_TYPE_EMAIL_VERIFY,
            &hash,
            "test@example.com",
            chrono::Utc::now() - chrono::Duration::seconds(1),
            None,
        )
        .await
        .unwrap();

    let (state, _sessions) = test_state();
    let state = state.with_magic_link_store(store);
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri(&format!("/auth/verify-email?token={}", urlencoding::encode(&raw)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn verify_email_redeem_cross_type_token_is_rejected() {
    // Magic-link login token stored at the same hash; the verify-email
    // route must not redeem it.
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let (raw, hash) = generate_magic_link_token();
    let expires = chrono::Utc::now() + chrono::Duration::minutes(15);
    store
        .store_token(TOKEN_TYPE_MAGIC_LINK, &hash, "test@example.com", expires, None)
        .await
        .unwrap();

    let (state, _sessions) = test_state();
    let state = state.with_magic_link_store(store.clone());
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri(&format!("/auth/verify-email?token={}", urlencoding::encode(&raw)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Original magic-link token is still consumable at its own route.
    let still_there = store
        .verify_and_consume(TOKEN_TYPE_MAGIC_LINK, &hash)
        .await
        .unwrap();
    assert_eq!(still_there.map(|c| c.email), Some("test@example.com".into()));
}

#[tokio::test]
async fn verify_email_resend_rate_limit_does_not_leak_state() {
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let (state, _sessions) = test_state();
    let state = state.with_magic_link_store(store.clone());
    let app = build_router(state.clone());

    // Submit 3 valid resend requests; all should render check-inbox.
    for _ in 0..3 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/verify-email/resend")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("email=test@example.com"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    // 4th request: rate-limited. Must still render check-inbox (no leak)
    // and must not store a 4th token.
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/verify-email/resend")
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
    assert!(html.contains("Verify your email"));

    let count = store
        .count_recent(
            TOKEN_TYPE_EMAIL_VERIFY,
            "test@example.com",
            chrono::Utc::now() - chrono::Duration::minutes(15),
        )
        .await
        .unwrap();
    assert_eq!(count, 3, "rate-limit must cap at 3");
}

#[tokio::test]
async fn add_email_with_verification_required_enqueues_email() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        add_email_result: Some(Ok(AddEmailResult {
            email: UserEmail {
                email: "secondary@understory.io".into(),
                verified: false,
            },
            email_verification_required: true,
        })),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(mock, MockPlatformClient::new());
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let state = state.with_magic_link_store(store.clone());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/account/emails")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "email=secondary@understory.io&_csrf=test-csrf",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    // Verification email enqueued under email-verify type.
    let count = store
        .count_recent(
            TOKEN_TYPE_EMAIL_VERIFY,
            "secondary@understory.io",
            chrono::Utc::now() - chrono::Duration::minutes(15),
        )
        .await
        .unwrap();
    assert_eq!(count, 1);
}

// ─── /settings/account "Try sending again" button ─────────────────────

#[tokio::test]
async fn account_resend_verification_enqueues_email_and_redirects_with_flash() {
    let (state, sessions) = test_state();
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let state = state.with_magic_link_store(store.clone());
    let cookie = create_test_session_unverified_email(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/account/emails/resend-verification")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("email=test@example.com&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get("location")
        .expect("redirect location")
        .to_str()
        .unwrap();
    assert_eq!(location, "/settings/account?flash=verification_resent");

    // A new email-verify token was enqueued.
    let count = store
        .count_recent(
            TOKEN_TYPE_EMAIL_VERIFY,
            "test@example.com",
            chrono::Utc::now() - chrono::Duration::minutes(15),
        )
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn account_resend_verification_for_already_verified_email_is_ineligible() {
    // The default session helper marks the email as verified. The
    // handler must refuse to re-send and never enqueue a token.
    let (state, sessions) = test_state();
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let state = state.with_magic_link_store(store.clone());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/account/emails/resend-verification")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("email=test@example.com&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get("location")
        .expect("redirect location")
        .to_str()
        .unwrap();
    assert_eq!(
        location,
        "/settings/account?error=verification_resend_ineligible"
    );

    // No token issued.
    let count = store
        .count_recent(
            TOKEN_TYPE_EMAIL_VERIFY,
            "test@example.com",
            chrono::Utc::now() - chrono::Duration::minutes(15),
        )
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn account_resend_verification_for_someone_elses_email_is_ineligible() {
    // Anti-abuse: even a logged-in user must not be able to trigger a
    // verification email for an address that isn't on their account.
    let (state, sessions) = test_state();
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let state = state.with_magic_link_store(store.clone());
    let cookie = create_test_session_unverified_email(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/account/emails/resend-verification")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("email=victim@example.com&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get("location")
        .expect("redirect location")
        .to_str()
        .unwrap();
    assert_eq!(
        location,
        "/settings/account?error=verification_resend_ineligible"
    );

    // Critically, no email is sent to the victim.
    let count = store
        .count_recent(
            TOKEN_TYPE_EMAIL_VERIFY,
            "victim@example.com",
            chrono::Utc::now() - chrono::Duration::minutes(15),
        )
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn account_resend_verification_rejects_bad_csrf() {
    let (state, sessions) = test_state();
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let state = state.with_magic_link_store(store.clone());
    let cookie = create_test_session_unverified_email(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/settings/account/emails/resend-verification")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("email=test@example.com&_csrf=wrong"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let count = store
        .count_recent(
            TOKEN_TYPE_EMAIL_VERIFY,
            "test@example.com",
            chrono::Utc::now() - chrono::Duration::minutes(15),
        )
        .await
        .unwrap();
    assert_eq!(count, 0);
}

// ─── DATA-251: verify-email carries return_to to /login ──────────────

/// When a sign-up with `return_to=/device?…` requires email verification,
/// the verification email's token is stored with `return_to`. Clicking
/// the link must consume the token and redirect to `/login?return_to=…`
/// instead of rendering the success page, so the user lands on the
/// device-approval screen once they sign in.
#[tokio::test]
async fn verify_email_redeem_with_return_to_redirects_to_login() {
    let store = std::sync::Arc::new(InMemoryMagicLinkStore::new());
    let (raw, hash) = generate_magic_link_token();
    let expires = chrono::Utc::now() + chrono::Duration::minutes(15);
    store
        .store_token(
            TOKEN_TYPE_EMAIL_VERIFY,
            &hash,
            "kasper@understory.io",
            expires,
            Some("/device?user_code=ABCD-EFGH"),
        )
        .await
        .unwrap();

    let (state, _sessions) = test_state();
    let state = state.with_magic_link_store(store);
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri(&format!(
                    "/auth/verify-email?token={}",
                    urlencoding::encode(&raw)
                ))
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
        location.starts_with("/login?return_to="),
        "expected /login redirect with return_to, got: {location}"
    );
    assert!(
        location.contains("ABCD-EFGH"),
        "user_code must survive into the login redirect: {location}"
    );
}

