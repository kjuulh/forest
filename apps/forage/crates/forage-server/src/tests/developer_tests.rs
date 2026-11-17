use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
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

    let body = "_csrf=test-csrf&name=My+App&description=&homepage_url=&redirect_uris=https%3A%2F%2Fapp.example%2Fcb&scope_profile=on";
    let resp = app
        .clone()
        .oneshot(post("/orgs/testorg/settings/developers", &cookie, body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    assert!(text.contains("Client secret created"));
    assert!(text.contains("forest_oas_secret_"));
    assert!(text.contains("forest_oa_app-1"));

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

    let body = "_csrf=test-csrf&name=OIDC+App&redirect_uris=https%3A%2F%2Fapp.example%2Fcb&scope_openid=on&scope_profile=on";
    app.clone()
        .oneshot(post("/orgs/testorg/settings/developers", &cookie, body))
        .await
        .unwrap();

    let resp = app
        .oneshot(get("/orgs/testorg/settings/developers", &cookie))
        .await
        .unwrap();
    let text = body_text(resp).await;
    assert!(text.contains("OIDC App"));
    assert!(text.contains("openid"));
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

    let body = "_csrf=test-csrf&name=My+App&redirect_uris=https%3A%2F%2Fapp.example%2Fcb&scope_profile=on";
    app.clone()
        .oneshot(post("/orgs/testorg/settings/developers", &cookie, body))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(get("/orgs/testorg/settings/developers/app-1", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = body_text(resp).await;
    assert!(text.contains("forest_oa_app-1"));
    assert!(!text.contains("Client secret created"));

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

    let resp = app
        .oneshot(get("/orgs/testorg/settings/developers/app-1", &cookie))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
