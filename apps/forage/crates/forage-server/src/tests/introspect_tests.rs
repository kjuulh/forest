//! Route tests for `POST /oauth/introspect` (RFC 7662).
//!
//! The caller is a resource server authenticating with its own
//! client-credentials token; the gate on THAT token (live, an app's own,
//! holding `tokens:introspect`) is what keeps this from being a token oracle.
//! The token asked about is an app's client token or an access token an app
//! holds for a user; a person's forest session or PAT is never active here.

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
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
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
    let (status, json) = post(
        Some("Bearer forest_cat_introspector"),
        "token=forest_cat_revoked",
    )
    .await;
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

#[tokio::test]
async fn a_client_token_answers_as_before_with_no_user_fields() {
    let (status, json) = post(
        Some("Bearer forest_cat_introspector"),
        "token=forest_cat_gateway_ci",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json,
        serde_json::json!({
            "active": true,
            "token_type": "bearer",
            "client_id": "app-1",
            "organisation_id": "org-1",
            "scope": "datagateway:apply datagateway:read",
        })
    );
}

#[tokio::test]
async fn a_user_delegated_oauth_token_introspects_as_that_user() {
    // `datactl` (a public client) signed kjuulh in with Forest and holds an
    // access token on their behalf, limited to what they consented to.
    let (status, json) = post(
        Some("Bearer forest_cat_introspector"),
        "token=forest_oat_datactl_kjuulh",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json,
        serde_json::json!({
            "active": true,
            "token_type": "bearer",
            "token_kind": "user",
            "client_id": "app-datactl",
            "organisation_id": "org-1",
            "scope": "datagateway:apply datagateway:read",
            "sub": "user-1",
            "username": "kjuulh",
            "organisations": [
                {"id": "org-1", "name": "testorg"},
                {"id": "org-2", "name": "understory"},
            ],
        })
    );
}

#[tokio::test]
async fn a_persons_forest_session_or_pat_is_inactive() {
    // `mock-access` is a live session in the mock forest. A person's own
    // session or personal access token carries everything they may do in
    // Forest; it is not handed to resource servers, so it does not
    // introspect, however live it is.
    for token in [
        "mock-access",
        "forest_pat_0123456789abcdef",
        "not-a-session",
    ] {
        let (status, json) = post(
            Some("Bearer forest_cat_introspector"),
            &format!("token={token}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{token}");
        assert_eq!(json, serde_json::json!({"active": false}), "{token}");
    }
}

#[tokio::test]
async fn a_dead_user_delegated_token_is_inactive() {
    let (status, json) = post(
        Some("Bearer forest_cat_introspector"),
        "token=forest_oat_revoked",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, serde_json::json!({"active": false}));
}

#[tokio::test]
async fn the_caller_must_be_an_app_itself_not_a_token_held_for_a_user() {
    // Even with `tokens:introspect` among its scopes, a token an app holds
    // on a person's behalf is not the resource server's own credential.
    let (status, json) = post(
        Some("Bearer forest_oat_holds_machine_scopes"),
        "token=forest_cat_gateway_ci",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(json["error"], "invalid_token");
}
