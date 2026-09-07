use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use tower::ServiceExt;

use crate::build_maintenance_router;
use crate::test_support::test_state;

#[tokio::test]
async fn maintenance_mode_replaces_all_application_routes() {
    let (state, _) = test_state();
    let app = build_maintenance_router(state.templates);

    for (method, uri) in [
        (Method::GET, "/"),
        (Method::GET, "/dashboard"),
        (Method::POST, "/oauth/token"),
        (Method::GET, "/a-route-that-does-not-exist"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{uri}");
        assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "300");
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Maintenance in progress"), "{uri}: {html}");
        assert!(
            html.contains("entire application is temporarily unavailable"),
            "{uri}: {html}"
        );
    }
}

#[tokio::test]
async fn maintenance_mode_keeps_liveness_probe_available() {
    let (state, _) = test_state();
    let response = build_maintenance_router(state.templates)
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
