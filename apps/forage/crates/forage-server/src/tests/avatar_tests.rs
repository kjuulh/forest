//! `/avatars/{identifier}` — the endpoint the release timeline points every
//! deployer's face at.
//!
//! A release knows who deployed it only by username, so the route has to accept
//! one, and most users have no uploaded picture at all — only the URL Google or
//! GitHub gave us at sign-in. These cover the resolution, and the ways it is
//! allowed to come up empty: the caller sees a 404 and the card falls back to
//! an initial, never a broken image.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::auth::{AuthError, UserProfile};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

fn profile(picture: Option<&str>) -> UserProfile {
    UserProfile {
        user_id: "user-123".into(),
        username: "kjuulh".into(),
        profile_picture_url: picture.map(|p| p.to_string()),
        created_at: Some("2025-01-15T10:00:00Z".into()),
    }
}

async fn get_avatar(behavior: MockBehavior, path: &str, with_session: bool) -> axum::response::Response {
    let (state, sessions) = test_state_with(
        MockForestClient::with_behavior(behavior),
        MockPlatformClient::new(),
    );
    let cookie = create_test_session(&sessions).await;
    let mut req = Request::builder().uri(path);
    if with_session {
        req = req.header("cookie", &cookie);
    }
    build_router(state)
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn username_redirects_to_the_picture_on_the_account() {
    let response = get_avatar(
        MockBehavior {
            get_user_by_username_result: Some(Ok(profile(Some(
                "https://lh3.googleusercontent.com/a/kjuulh",
            )))),
            ..Default::default()
        },
        "/avatars/kjuulh",
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "https://lh3.googleusercontent.com/a/kjuulh"
    );
}

#[tokio::test]
async fn username_without_a_session_is_not_found() {
    let response = get_avatar(
        MockBehavior {
            get_user_by_username_result: Some(Ok(profile(Some("https://example.com/pic.png")))),
            ..Default::default()
        },
        "/avatars/kjuulh",
        false,
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn user_with_no_picture_is_not_found() {
    let response = get_avatar(
        MockBehavior {
            get_user_by_username_result: Some(Ok(profile(None))),
            ..Default::default()
        },
        "/avatars/kjuulh",
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_user_is_not_found() {
    let response = get_avatar(
        MockBehavior {
            get_user_by_username_result: Some(Err(AuthError::NotFound)),
            ..Default::default()
        },
        "/avatars/nobody",
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// An upload records its own `/avatars/{user_id}` URL on the account. Reaching
/// this point means that upload could not be served, so following the URL would
/// come straight back here — 404 instead of looping the browser.
#[tokio::test]
async fn a_picture_url_pointing_back_here_is_not_followed() {
    let response = get_avatar(
        MockBehavior {
            get_user_by_username_result: Some(Ok(profile(Some(
                "https://forage.example.com/avatars/user-123",
            )))),
            ..Default::default()
        },
        "/avatars/kjuulh",
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Only http(s) is ever handed back to the browser as a redirect target.
#[tokio::test]
async fn a_non_http_picture_url_is_not_followed() {
    let response = get_avatar(
        MockBehavior {
            get_user_by_username_result: Some(Ok(profile(Some("javascript:alert(1)")))),
            ..Default::default()
        },
        "/avatars/kjuulh",
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Anything that is not a username never reaches the user lookup.
#[tokio::test]
async fn an_identifier_that_cannot_be_a_username_is_not_found() {
    let response = get_avatar(
        MockBehavior {
            get_user_by_username_result: Some(Ok(profile(Some("https://example.com/pic.png")))),
            ..Default::default()
        },
        "/avatars/ab",
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
