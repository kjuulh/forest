//! Route tests for `/api/directory/resolve`.
//!
//! This is the only endpoint in Forage authenticated by a machine token
//! rather than a session, so the gate itself is the thing most worth
//! covering: everything else on this router would 302 an anonymous
//! caller to a login page, and this one must 401 instead.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

const TOKEN: &str = "forest_cat_testtoken";

async fn body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

async fn get(uri: &str, auth: Option<&str>) -> (StatusCode, serde_json::Value) {
    let (state, _sessions) = test_state_with_oauth_apps(Arc::new(MockOAuthAppsClient::new()));
    let app = build_router(state);

    let mut req = Request::builder().method("GET").uri(uri);
    if let Some(a) = auth {
        req = req.header("authorization", a);
    }
    let resp = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    let body = body_text(resp).await;
    let json = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn without_a_token_it_401s_rather_than_redirecting_to_login() {
    let (status, json) = get("/api/directory/resolve?email=a@b.test", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"], "invalid_token");
}

#[tokio::test]
async fn a_non_bearer_authorization_header_is_not_accepted() {
    let (status, _) = get(
        "/api/directory/resolve?email=a@b.test",
        Some("Basic dXNlcjpwYXNz"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn resolves_a_person_by_github_identity() {
    let (status, json) = get(
        "/api/directory/resolve?provider=github&provider_user_id=26280046",
        Some(&format!("Bearer {TOKEN}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["found"], true);
    assert_eq!(json["username"], "kjuulh");
    assert_eq!(json["user_id"], "user-1");
}

#[tokio::test]
async fn resolves_a_person_by_email() {
    let (status, json) = get(
        "/api/directory/resolve?email=kasper@understory.io",
        Some(&format!("Bearer {TOKEN}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["found"], true);
}

/// Most commit authors will never have a Forest account, so a miss is a
/// normal answer — 200 with `found: false`, not a 404. A caller looping
/// over people shouldn't have to treat "not our colleague" as an error.
#[tokio::test]
async fn an_unknown_person_is_a_200_with_found_false() {
    let (status, json) = get(
        "/api/directory/resolve?email=stranger@example.test",
        Some(&format!("Bearer {TOKEN}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["found"], false);
}

#[tokio::test]
async fn a_malformed_query_says_what_is_wrong() {
    let auth = format!("Bearer {TOKEN}");
    for uri in [
        "/api/directory/resolve",
        "/api/directory/resolve?provider=github",
        "/api/directory/resolve?email=a@b.test&provider=github&provider_user_id=1",
    ] {
        let (status, json) = get(uri, Some(&auth)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(json["error"], "invalid_request", "{uri}");
        assert!(
            json["error_description"].as_str().is_some_and(|d| !d.is_empty()),
            "{uri} should explain itself"
        );
    }
}
