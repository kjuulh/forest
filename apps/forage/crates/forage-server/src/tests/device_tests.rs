use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::session::InMemorySessionStore;
use tower::ServiceExt;

use crate::build_router;
use crate::state::AppState;
use crate::test_support::*;

fn configured_state(
    mock: MockForestClient,
) -> (AppState, Arc<InMemorySessionStore>) {
    let (state, sessions) = test_state_with(mock, MockPlatformClient::new());
    (
        state.with_service_account_key("test-service-key".into()),
        sessions,
    )
}

#[tokio::test]
async fn device_get_without_session_redirects_to_login() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/device?user_code=ABCD-EFGH")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "expected redirect, got {:?}",
        response.status()
    );
    let location = response
        .headers()
        .get("location")
        .expect("redirect location")
        .to_str()
        .unwrap();
    assert!(
        location.starts_with("/login?return_to="),
        "redirect should preserve return_to: {location}"
    );
    assert!(
        location.contains("user_code"),
        "user_code must be carried through to /login: {location}"
    );
}

#[tokio::test]
async fn device_get_with_session_prefills_user_code() {
    let (state, sessions) = configured_state(MockForestClient::new());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/device?user_code=ABCD-EFGH")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response).await;
    assert!(html.contains("ABCD-EFGH"), "user_code prefilled: {html}");
    assert!(
        html.contains("Approve") && html.contains("Deny"),
        "approve/deny buttons visible"
    );
    assert!(
        html.contains("Only approve if you started this"),
        "warning copy must be present"
    );
}

#[tokio::test]
async fn device_get_without_user_code_still_renders() {
    let (state, sessions) = configured_state(MockForestClient::new());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/device")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response).await;
    assert!(html.contains(r#"name="user_code""#));
}

#[tokio::test]
async fn device_approve_calls_forest_and_shows_success() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        approve_device_login_result: Some(Ok(())),
        ..Default::default()
    });
    let (state, sessions) = configured_state(mock);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let body = "user_code=ABCD-EFGH&action=approve&_csrf=test-csrf";
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/device")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response).await;
    assert!(
        html.contains("You're signed in") || html.contains("signed in"),
        "success message must render: {html}"
    );
}

#[tokio::test]
async fn device_post_with_bad_csrf_is_rejected() {
    let (state, sessions) = configured_state(MockForestClient::new());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let body = "user_code=ABCD-EFGH&action=approve&_csrf=wrong-csrf";
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/device")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn device_deny_calls_forest_and_shows_cancelled() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        deny_device_login_result: Some(Ok(())),
        ..Default::default()
    });
    let (state, sessions) = configured_state(mock);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let body = "user_code=ABCD-EFGH&action=deny&_csrf=test-csrf";
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/device")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response).await;
    assert!(
        html.contains("denied") || html.contains("cancelled"),
        "deny outcome must render: {html}"
    );
}

#[tokio::test]
async fn device_post_with_unknown_action_returns_400() {
    let (state, sessions) = configured_state(MockForestClient::new());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let body = "user_code=ABCD-EFGH&action=please-pwn&_csrf=test-csrf";
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/device")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn device_post_with_empty_code_rerenders_form() {
    let (state, sessions) = configured_state(MockForestClient::new());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let body = "user_code=&action=approve&_csrf=test-csrf";
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/device")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response).await;
    assert!(
        html.contains("Enter the code"),
        "empty-code prompt must show: {html}"
    );
}

#[tokio::test]
async fn device_approve_when_forest_rejects_shows_friendly_error() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        approve_device_login_result: Some(Err(
            forage_core::auth::AuthError::Other("expired".into()),
        )),
        ..Default::default()
    });
    let (state, sessions) = configured_state(mock);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let body = "user_code=ABCD-EFGH&action=approve&_csrf=test-csrf";
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/device")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response).await;
    assert!(
        html.contains("wasn't recognised") || html.contains("expired"),
        "user-friendly error must render: {html}"
    );
    assert!(
        !html.contains("Other("),
        "raw AuthError debug repr must not leak to the user: {html}"
    );
}

#[tokio::test]
async fn device_get_without_service_account_key_shows_explicit_unconfigured_message() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/device?user_code=ABCD-EFGH")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response).await;
    assert!(
        html.contains("isn't configured") || html.contains("not configured"),
        "must say the feature isn't configured: {html}"
    );
    assert!(
        html.contains("FOREST_SERVICE_ACCOUNT_API_KEY"),
        "must name the missing env var so the operator can fix it: {html}"
    );
    assert!(
        html.contains("forest auth login --password"),
        "must give users a working fallback: {html}"
    );
    assert!(
        !html.contains(r#"name="action""#),
        "approve/deny form must NOT render when unconfigured: {html}"
    );
}

#[tokio::test]
async fn device_post_without_service_account_key_does_not_call_forest() {
    let mock = MockForestClient::with_behavior(MockBehavior {
        approve_device_login_result: Some(Err(
            forage_core::auth::AuthError::Other("should never be called".into()),
        )),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(mock, MockPlatformClient::new());
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let body = "user_code=ABCD-EFGH&action=approve&_csrf=test-csrf";
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/device")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_to_string(response).await;
    assert!(
        html.contains("isn't configured") || html.contains("not configured"),
        "POST must surface the same unconfigured message: {html}"
    );
    assert!(
        !html.contains("should never be called"),
        "forest_client.approve_device_login must not be called when key is missing"
    );
}

async fn body_to_string(response: axum::http::Response<Body>) -> String {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}
