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
