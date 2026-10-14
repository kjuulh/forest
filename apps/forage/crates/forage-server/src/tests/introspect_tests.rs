//! Route tests for `POST /oauth/introspect` (RFC 7662).
//!
//! The caller is a resource server authenticating with its own
//! client-credentials token; the gate on THAT token (live, holding
//! `tokens:introspect`) is what keeps this from being a token oracle.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

async fn post(auth: Option<&str>, body: &str) -> (StatusCode, serde_json::Value) {
    let (state, _sessions) = test_state_with_oauth_apps(Arc::new(MockOAuthAppsClient::new()));
    let app = build_router(state);
    let mut req = Request::builder()
        .method("POST")
        .uri("/oauth/introspect")
        .header("content-type", "application/x-www-form-urlencoded");
    if let Some(a) = auth {
        req = req.header("authorization", a);
    }
    let resp = app
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn an_introspector_learns_a_live_tokens_app_org_and_scopes() {
    let (status, json) = post(
        Some("Bearer forest_cat_introspector"),
        "token=forest_cat_gateway_ci",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["active"], true);
    assert_eq!(json["client_id"], "app-1");
    assert_eq!(json["organisation_id"], "org-1");
    assert_eq!(json["scope"], "datagateway:apply datagateway:read");
}

#[tokio::test]
async fn a_dead_token_is_inactive_and_nothing_more() {
    let (status, json) =
        post(Some("Bearer forest_cat_introspector"), "token=forest_cat_revoked").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, serde_json::json!({"active": false}));
}

#[tokio::test]
async fn the_caller_must_hold_tokens_introspect() {
    // A live token without the scope (here a directory client) may not ask.
    let (status, json) = post(
        Some("Bearer forest_cat_testtoken"),
        "token=forest_cat_gateway_ci",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(json["error"], "insufficient_scope");
}

#[tokio::test]
async fn an_unauthenticated_caller_is_401_not_a_login_redirect() {
    let (status, json) = post(None, "token=forest_cat_gateway_ci").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"], "invalid_token");
    let (status, _) = post(Some("Bearer forest_cat_revoked"), "token=x").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_missing_token_parameter_is_a_bad_request() {
    let (status, json) = post(Some("Bearer forest_cat_introspector"), "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"], "invalid_request");
}
