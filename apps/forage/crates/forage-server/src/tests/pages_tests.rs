use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::test_support::*;

#[tokio::test]
async fn landing_page_returns_200() {
    let response = test_app()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn landing_page_contains_expected_content() {
    let response = test_app()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Forage - The Platform for Forest"));
    assert!(html.contains("forest.cue"));
    assert!(html.contains("Component Registry"));
    assert!(html.contains("Managed Deployments"));
    assert!(html.contains("Container Deployments"));
}

#[tokio::test]
async fn pricing_page_returns_200() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/pricing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn pricing_page_contains_all_tiers() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/pricing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Free"));
    assert!(html.contains("Developer"));
    assert!(html.contains("Team"));
    assert!(html.contains("Enterprise"));
    assert!(html.contains("$10"));
    assert!(html.contains("$25"));
}

#[tokio::test]
async fn landing_page_redirects_to_dashboard_when_authenticated() {
    let (state, sessions) = test_state();
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get("location").unwrap().to_str().unwrap(),
        "/dashboard"
    );
}

#[tokio::test]
async fn unknown_route_returns_404() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
