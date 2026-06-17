//! Route tests for Developer Settings (org OAuth applications, M1).
//!
//! The forest-server OAuth-apps client is mocked in-memory, so these cover the
//! Forage HTTP surface: rendering, CSRF, admin authorization, and the
//! create → list → detail → rotate → delete lifecycle wiring.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::PlatformError;
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

fn get(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap()
}

fn post(uri: &str, cookie: &str, body: &'static str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("cookie", cookie)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn list_empty_shows_register_cta() {
    let (state, sessions) = test_state_with_oauth_apps(Arc::new(MockOAuthAppsClient::new()));
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let resp = app
        .oneshot(get("/orgs/testorg/settings/developers", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    assert!(text.contains("Developer settings"));
    assert!(text.contains("No OAuth applications"));
}

#[tokio::test]
async fn create_shows_secret_once_then_lists_app() {
    let oauth = Arc::new(MockOAuthAppsClient::new());
    let (state, sessions) = test_state_with_oauth_apps(oauth);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    // Create
    let body = "_csrf=test-csrf&name=My+App&description=&homepage_url=&redirect_uris=https%3A%2F%2Fapp.example%2Fcb&scope_profile=on";
    let resp = app
        .clone()
        .oneshot(post("/orgs/testorg/settings/developers", &cookie, body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    // The client_secret is shown exactly once on the create response.
    assert!(text.contains("Client secret created"));
    assert!(text.contains("forest_oas_secret_"));
    assert!(text.contains("forest_oa_app-1")); // client_id

    // List now shows the app, and never the secret.
    let resp = app
        .oneshot(get("/orgs/testorg/settings/developers", &cookie))
        .await
        .unwrap();
    let text = body_text(resp).await;
    assert!(text.contains("My App"));
    assert!(!text.contains("forest_oas_secret_"));
}

#[tokio::test]
async fn create_with_openid_scope_is_requestable() {
    let oauth = Arc::new(MockOAuthAppsClient::new());
    let (state, sessions) = test_state_with_oauth_apps(oauth);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    // Tick openid + profile in the create form.
    let body = "_csrf=test-csrf&name=OIDC+App&redirect_uris=https%3A%2F%2Fapp.example%2Fcb&scope_openid=on&scope_profile=on";
    app.clone()
        .oneshot(post("/orgs/testorg/settings/developers", &cookie, body))
        .await
        .unwrap();

    // The app is registered with the `openid` scope (shown as a chip on the list).
    let resp = app
        .oneshot(get("/orgs/testorg/settings/developers", &cookie))
        .await
        .unwrap();
    let text = body_text(resp).await;
    assert!(text.contains("OIDC App"));
    assert!(text.contains("openid"));
}

#[tokio::test]
async fn create_validation_error_redirects_back_with_message() {
    let oauth = Arc::new(MockOAuthAppsClient::with_error(
        PlatformError::InvalidArgument("invalid redirect URI: http://app.example/cb".into()),
    ));
    let (state, sessions) = test_state_with_oauth_apps(oauth);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let body = "_csrf=test-csrf&name=My+App&redirect_uris=http%3A%2F%2Fapp.example%2Fcb&scope_profile=on";
    let resp = app
        .oneshot(post("/orgs/testorg/settings/developers", &cookie, body))
        .await
        .unwrap();
    // Redirects back to the form with the error surfaced as a query param.
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(loc.starts_with("/orgs/testorg/settings/developers/new?error="));
    assert!(loc.contains("invalid%20redirect"));
}

#[tokio::test]
async fn create_with_bad_csrf_is_rejected() {
    let (state, sessions) = test_state_with_oauth_apps(Arc::new(MockOAuthAppsClient::new()));
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let body = "_csrf=wrong&name=My+App&redirect_uris=https%3A%2F%2Fapp.example%2Fcb&scope_profile=on";
    let resp = app
        .oneshot(post("/orgs/testorg/settings/developers", &cookie, body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn non_admin_member_is_denied() {
    let (state, sessions) = test_state_with_oauth_apps(Arc::new(MockOAuthAppsClient::new()));
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let resp = app
        .oneshot(get("/orgs/testorg/settings/developers", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn detail_rotate_and_delete_lifecycle() {
    let oauth = Arc::new(MockOAuthAppsClient::new());
    let (state, sessions) = test_state_with_oauth_apps(oauth);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    // Create one (app-1).
    let body = "_csrf=test-csrf&name=My+App&redirect_uris=https%3A%2F%2Fapp.example%2Fcb&scope_profile=on";
    app.clone()
        .oneshot(post("/orgs/testorg/settings/developers", &cookie, body))
        .await
        .unwrap();

    // Detail renders the client_id, no secret.
    let resp = app
        .clone()
        .oneshot(get("/orgs/testorg/settings/developers/app-1", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    assert!(text.contains("forest_oa_app-1"));
    assert!(!text.contains("Client secret created"));

    // Rotate → new secret shown once.
    let resp = app
        .clone()
        .oneshot(post(
            "/orgs/testorg/settings/developers/app-1/rotate",
            &cookie,
            "_csrf=test-csrf",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    assert!(text.contains("forest_oas_rotated_app-1"));

    // Delete → redirects to the list.
    let resp = app
        .clone()
        .oneshot(post(
            "/orgs/testorg/settings/developers/app-1/delete",
            &cookie,
            "_csrf=test-csrf",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get("location").unwrap().to_str().unwrap(),
        "/orgs/testorg/settings/developers"
    );

    // It's gone → detail now 404s.
    let resp = app
        .oneshot(get("/orgs/testorg/settings/developers/app-1", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
