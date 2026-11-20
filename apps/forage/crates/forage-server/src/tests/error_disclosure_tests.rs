use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use forage_core::platform::PlatformError;
use tower::ServiceExt;

use crate::{build_router, test_support::*};

const DETAIL: &str = "gRPC Internal at db.internal.test: SQL SELECT private_records; password=canary_password https://operator:canary_url_secret@db.internal.test/private?token=canary_query";

fn assert_no_internal_detail(body: &str) {
    for marker in [
        "gRPC",
        "db.internal.test",
        "SELECT",
        "private_records",
        "canary_",
        "operator:",
    ] {
        assert!(!body.contains(marker), "internal detail exposed: {marker}");
    }
}

fn reference(body: &str) -> uuid::Uuid {
    body.split(|c: char| !c.is_ascii_hexdigit() && c != '-')
        .filter_map(|word| uuid::Uuid::parse_str(word).ok())
        .find(|id| id.get_version_num() == 4)
        .expect("failed dependency needs a server correlation reference")
}

#[tokio::test]
async fn dependency_failures_hide_diagnostics_and_link_body_to_a_fresh_header_reference() {
    let caller_id = uuid::Uuid::new_v4();
    let mut seen = std::collections::HashSet::new();
    for error in [
        PlatformError::Unavailable(DETAIL.into()),
        PlatformError::Other(DETAIL.into()),
    ] {
        let (state, sessions) = test_state_with(
            MockForestClient::new(),
            MockPlatformClient::with_behavior(MockPlatformBehavior {
                list_projects_result: Some(Err(error)),
                ..Default::default()
            }),
        );
        let cookie = create_test_session(&sessions).await;
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .uri("/orgs/testorg/projects")
                    .header("cookie", cookie)
                    .header("x-error-id", caller_id.to_string())
                    .header("x-request-id", caller_id.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let id = uuid::Uuid::parse_str(response.headers()["x-error-id"].to_str().unwrap()).unwrap();
        assert_eq!(id.get_version_num(), 4);
        assert_ne!(id, caller_id);
        assert!(seen.insert(id));
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = std::str::from_utf8(&body).unwrap();
        assert_no_internal_detail(html);
        assert!(html.contains(&id.to_string()));
    }
}

#[tokio::test]
async fn approval_failures_are_generic_for_html_and_json_without_changing_status() {
    for uri in [
        "/orgs/testorg/projects/my-api/releases/test-release/approve",
        "/orgs/testorg/projects/my-api/releases/test-release/reject",
        "/api/orgs/testorg/projects/my-api/plan-stages/review/approve",
        "/api/orgs/testorg/projects/my-api/plan-stages/review/reject",
    ] {
        for accept in ["text/html", "application/json"] {
            let (state, sessions) = test_state_with(
                MockForestClient::new(),
                MockPlatformClient::with_behavior(MockPlatformBehavior {
                    approval_error: Some(PlatformError::Other(DETAIL.into())),
                    ..Default::default()
                }),
            );
            let cookie = create_test_session(&sessions).await;
            let response = build_router(state).oneshot(Request::builder().method("POST").uri(uri)
                .header("cookie", cookie).header("accept", accept)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("csrf_token=test-csrf&release_intent_id=isolated&target_environment=prod")).unwrap()).await.unwrap();
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let body = std::str::from_utf8(&body).unwrap();
            assert_no_internal_detail(body);
            reference(body);
            if accept == "application/json" {
                let json: serde_json::Value = serde_json::from_str(body).unwrap();
                assert!(json["error"].as_str().is_some());
            }
        }
    }
}

#[tokio::test]
async fn backend_validation_payloads_are_not_echoed_through_organisation_forms() {
    for error in [
        PlatformError::Unavailable(DETAIL.into()),
        PlatformError::InvalidArgument(DETAIL.into()),
        PlatformError::PermissionDenied(DETAIL.into()),
        PlatformError::NotFound(DETAIL.into()),
    ] {
        let (state, sessions) = test_state_with(
            MockForestClient::new(),
            MockPlatformClient::with_behavior(MockPlatformBehavior {
                create_organisation_result: Some(Err(error)),
                ..Default::default()
            }),
        );
        let cookie = create_test_session(&sessions).await;
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/orgs")
                    .header("cookie", cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=valid-org&_csrf=test-csrf"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = std::str::from_utf8(&body).unwrap();
        assert_no_internal_detail(html);
        reference(html);
    }
}

#[tokio::test]
async fn developer_validation_errors_keep_the_redirect_flow_without_forwarding_backend_text() {
    let oauth = std::sync::Arc::new(MockOAuthAppsClient::with_error(
        PlatformError::InvalidArgument(DETAIL.into()),
    ));
    let (state, sessions) = test_state_with_oauth_apps(oauth);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);
    let response = app.clone().oneshot(Request::builder().method("POST")
        .uri("/orgs/testorg/settings/developers")
        .header("cookie", &cookie).header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("_csrf=test-csrf&name=My+App&redirect_uris=https%3A%2F%2Fapp.example%2Fcb&scope_profile=on"))
        .unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    assert!(location.starts_with("/orgs/testorg/settings/developers/new?error="));
    let decoded = urlencoding::decode(&location).unwrap();
    assert_no_internal_detail(&decoded);
    let id = reference(&decoded);
    let response = app
        .oneshot(
            Request::builder()
                .uri(&location)
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = std::str::from_utf8(&body).unwrap();
    assert_no_internal_detail(html);
    assert!(html.contains(&id.to_string()));
}

#[tokio::test]
async fn domain_backend_errors_keep_inline_forms_without_disclosing_backend_details() {
    for (uri, behavior) in [
        (
            "/orgs/testorg/settings/access",
            MockPlatformBehavior {
                add_allowed_domain_result: Some(Err(PlatformError::InvalidArgument(DETAIL.into()))),
                ..Default::default()
            },
        ),
        (
            "/orgs/testorg/settings/access/verify",
            MockPlatformBehavior {
                verify_allowed_domain_result: Some(Err(PlatformError::Unavailable(DETAIL.into()))),
                ..Default::default()
            },
        ),
    ] {
        let (state, sessions) = test_state_with(
            MockForestClient::new(),
            MockPlatformClient::with_behavior(behavior),
        );
        let cookie = create_test_session(&sessions).await;
        let response = build_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("cookie", cookie)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("domain=corp.example&_csrf=test-csrf"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = std::str::from_utf8(&body).unwrap();
        assert_no_internal_detail(html);
        reference(html);
    }
}

#[tokio::test]
async fn slack_denial_redirect_does_not_forward_untrusted_provider_error_text() {
    use forage_core::auth::oauth_state::{InMemoryOAuthStateStore, OAuthStateStore};

    let (state, sessions, _) =
        test_state_with_integrations(MockForestClient::new(), MockPlatformClient::new());
    let states = std::sync::Arc::new(InMemoryOAuthStateStore::new());
    states
        .create(
            "slack_install",
            "nonce",
            Some("testorg\nuser-123\n"),
            chrono::Utc::now() + chrono::Duration::minutes(10),
        )
        .await
        .unwrap();
    let cookie = create_test_session(&sessions).await;
    let response = build_router(state.with_oauth_state_store(states))
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/integrations/slack/callback?state=nonce&error={}",
                    urlencoding::encode(DETAIL)
                ))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = urlencoding::decode(response.headers()["location"].to_str().unwrap()).unwrap();
    assert!(location.starts_with("/orgs/testorg/settings/integrations/install/slack?error="));
    assert_no_internal_detail(&location);
}

#[tokio::test]
async fn visibility_refusals_keep_their_status_without_forwarding_backend_text() {
    for uri in [
        "/orgs/testorg/settings/visibility",
        "/orgs/testorg/projects/widget/settings/visibility",
    ] {
        for (error, status) in [
            (
                PlatformError::PermissionDenied(DETAIL.into()),
                StatusCode::FORBIDDEN,
            ),
            (
                PlatformError::InvalidArgument(DETAIL.into()),
                StatusCode::BAD_REQUEST,
            ),
            (
                PlatformError::NotFound(DETAIL.into()),
                StatusCode::NOT_FOUND,
            ),
        ] {
            let (state, sessions) = test_state_with(
                MockForestClient::new(),
                MockPlatformClient::with_behavior(MockPlatformBehavior {
                    get_visibility_result: Some(Ok(forage_core::platform::Visibility {
                        organisation: "private".into(),
                        project: uri.contains("/projects/").then(|| "private".into()),
                        can_change: true,
                    })),
                    set_visibility_result: Some(Err(error)),
                    ..Default::default()
                }),
            );
            let cookie = create_test_session(&sessions).await;
            let response = build_router(state)
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header("cookie", cookie)
                        .header("content-type", "application/x-www-form-urlencoded")
                        .body(Body::from("visibility=public&_csrf=test-csrf"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{uri}");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let html = std::str::from_utf8(&body).unwrap();
            assert_no_internal_detail(html);
            reference(html);
        }
    }
}
