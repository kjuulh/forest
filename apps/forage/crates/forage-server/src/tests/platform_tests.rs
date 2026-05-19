use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::{
    Artifact, ArtifactContext, ArtifactDestination, ArtifactRef, ArtifactSource, PlatformError,
};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

// ─── Dashboard ─────────────────────────────────────────────────────

#[tokio::test]
async fn dashboard_with_orgs_shows_dashboard_page() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("testorg"));
    assert!(html.contains("Recent activity"));
}

#[tokio::test]
async fn dashboard_shows_recent_artifacts() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Deploy v1.0"));
}

#[tokio::test]
async fn dashboard_empty_activity_shows_empty_state() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_projects_result: Some(Ok(vec!["my-api".into()])),
        list_artifacts_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("No recent activity"));
}

#[tokio::test]
async fn dashboard_no_orgs_shows_onboarding() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_orgs_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session_no_orgs(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Create organisation"));
}

// ─── Create organisation ───────────────────────────────────────────

#[tokio::test]
async fn create_org_success_redirects_to_new_org() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=my-new-org&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "/orgs/my-new-org/projects"
    );
}

#[tokio::test]
async fn create_org_invalid_slug_shows_error() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=INVALID ORG&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("invalid") || html.contains("Invalid"));
}

#[tokio::test]
async fn create_org_invalid_csrf_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=my-org&_csrf=wrong-token"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_org_grpc_failure_shows_error() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        create_organisation_result: Some(Err(PlatformError::Unavailable(
            "connection refused".into(),
        ))),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=my-org&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        html.contains("unavailable") || html.contains("error") || html.contains("try again")
    );
}

// ─── Members page ──────────────────────────────────────────────────

#[tokio::test]
async fn members_page_returns_200_with_members() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/members")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("testuser"));
    assert!(html.contains("owner"));
}

#[tokio::test]
async fn members_page_non_member_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/unknown-org/settings/members")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn members_page_invalid_slug_returns_400() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/INVALID%20ORG/settings/members")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn members_page_unauthenticated_redirects() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/members")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(response.headers().get("location").unwrap().to_str().unwrap().starts_with("/login"));
}

// ─── Member management ─────────────────────────────────────────────

#[tokio::test]
async fn add_member_success_redirects() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/members")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "username=newuser&role=member&_csrf=test-csrf",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "/orgs/testorg/settings/members"
    );
}

#[tokio::test]
async fn add_member_invalid_csrf_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/members")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "username=newuser&role=member&_csrf=wrong-token",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn remove_member_success_redirects() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/members/user-456/remove")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "/orgs/testorg/settings/members"
    );
}

#[tokio::test]
async fn update_member_role_success_redirects() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/members/user-456/role")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("role=admin&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "/orgs/testorg/settings/members"
    );
}

#[tokio::test]
async fn add_member_non_admin_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/members")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "username=newuser&role=member&_csrf=test-csrf",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn remove_member_non_admin_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/members/user-456/remove")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn update_role_non_admin_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/settings/members/user-456/role")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("role=admin&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn members_page_non_admin_can_view() {
    let (state, sessions) = test_state();
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/members")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    // Can see members but NOT the add member form
    assert!(html.contains("testuser"));
    assert!(!html.contains("Add member"));
}

// ─── Projects list ──────────────────────────────────────────────────

#[tokio::test]
async fn projects_list_returns_200_with_projects() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("my-api"));
}

#[tokio::test]
async fn projects_list_empty_shows_empty_state() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_projects_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("No projects yet"));
}

#[tokio::test]
async fn projects_list_unauthenticated_redirects() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(response.headers().get("location").unwrap().to_str().unwrap().starts_with("/login"));
}

#[tokio::test]
async fn projects_list_non_member_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/unknown-org/projects")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn projects_list_platform_unavailable_returns_500() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_projects_result: Some(Err(PlatformError::Unavailable(
            "connection refused".into(),
        ))),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Something went wrong"));
    assert!(html.contains("connection refused"));
}

// ─── Project detail ─────────────────────────────────────────────────

// Per specs/features/008, the project Overview at `/orgs/{org}/projects/{project}`
// no longer renders the `<release-timeline>` element — that moved to the
// new Deployments tab at `/orgs/{org}/projects/{project}/releases`. These
// three tests hit the Deployments URL so they keep asserting the timeline's
// presence at the right place.

#[tokio::test]
async fn project_releases_returns_200_with_artifacts() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/releases")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    // The timeline is now rendered by a Svelte web component
    assert!(html.contains("release-timeline"));
    assert!(html.contains("org=\"testorg\""));
    assert!(html.contains("project=\"my-api\""));
}

#[tokio::test]
async fn project_releases_empty_artifacts_shows_empty_state() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_artifacts_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/releases")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    // Empty state is now rendered client-side by the Svelte component
    assert!(html.contains("release-timeline"));
    assert!(html.contains("project=\"my-api\""));
}

#[tokio::test]
async fn project_releases_shows_enriched_artifact_data() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_artifacts_result: Some(Ok(vec![Artifact {
            artifact_id: "art-2".into(),
            slug: "my-api-def456".into(),
            context: ArtifactContext {
                title: "Deploy v2.0".into(),
                description: Some("Major release".into()),
                web: None,
                pr: None,
            },
            source: Some(ArtifactSource {
                user: Some("ci-bot".into()),
                email: None,
                source_type: Some("github-actions".into()),
                run_url: Some("https://github.com/org/repo/actions/runs/123".into()),
            }),
            git_ref: Some(ArtifactRef {
                commit_sha: "abc1234".into(),
                branch: Some("main".into()),
                commit_message: Some("feat: add new feature".into()),
                version: Some("v2.0.0".into()),
                repo_url: None,
            }),
            destinations: vec![ArtifactDestination {
                name: "production".into(),
                environment: "prod".into(),
                type_organisation: None,
                type_name: None,
                type_version: None,
                status: None,
            }],
            created_at: "2026-03-07T12:00:00Z".into(),
        }])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/releases")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    // Enriched data is now rendered client-side by the Svelte component
    assert!(html.contains("release-timeline"));
    assert!(html.contains("project=\"my-api\""));
}

#[tokio::test]
async fn timeline_api_returns_json_with_artifacts() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/orgs/testorg/projects/my-api/timeline")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["timeline"].is_array());
    assert!(json["lanes"].is_array());
    // Should have at least one timeline item from the mock data
    assert!(!json["timeline"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn org_timeline_api_returns_json() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/orgs/testorg/timeline")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["timeline"].is_array());
    assert!(json["lanes"].is_array());
}

#[tokio::test]
async fn timeline_api_requires_auth() {
    let (state, _sessions) = test_state();
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/orgs/testorg/projects/my-api/timeline")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Should redirect to login (302) when not authenticated
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

// ─── Artifact detail ────────────────────────────────────────────────

#[tokio::test]
async fn artifact_detail_returns_200() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/releases/my-api-abc123")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("my-api-abc123"));
    assert!(html.contains("Deploy v1.0"));
}

#[tokio::test]
async fn artifact_detail_shows_enriched_data() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        get_artifact_by_slug_result: Some(Ok(Artifact {
            artifact_id: "art-2".into(),
            slug: "my-api-def456".into(),
            context: ArtifactContext {
                title: "Deploy v2.0".into(),
                description: Some("Major release".into()),
                web: Some("https://example.com".into()),
                pr: Some("https://github.com/org/repo/pull/42".into()),
            },
            source: Some(ArtifactSource {
                user: Some("ci-bot".into()),
                email: Some("ci@example.com".into()),
                source_type: Some("github-actions".into()),
                run_url: Some("https://github.com/org/repo/actions/runs/123".into()),
            }),
            git_ref: Some(ArtifactRef {
                commit_sha: "abc1234".into(),
                branch: Some("main".into()),
                commit_message: Some("feat: add new feature".into()),
                version: Some("v2.0.0".into()),
                repo_url: Some("https://github.com/org/repo".into()),
            }),
            destinations: vec![
                ArtifactDestination {
                    name: "production".into(),
                    environment: "prod".into(),
                    type_organisation: None,
                    type_name: None,
                    type_version: None,
                    status: None,
                },
                ArtifactDestination {
                    name: "staging".into(),
                    environment: "staging".into(),
                    type_organisation: None,
                    type_name: None,
                    type_version: None,
                    status: None,
                },
            ],
            created_at: "2026-03-07T12:00:00Z".into(),
        })),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/releases/my-api-def456")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("v2.0.0"));
    assert!(html.contains("main"));
    assert!(html.contains("abc1234"));
    assert!(html.contains("ci-bot"));
    assert!(html.contains("production"));
    assert!(html.contains("staging"));
    assert!(html.contains("Major release"));
}

#[tokio::test]
async fn artifact_detail_not_found_returns_404() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        get_artifact_by_slug_result: Some(Err(PlatformError::NotFound(
            "artifact not found".into(),
        ))),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/releases/nonexistent")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn artifact_detail_unauthenticated_redirects() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/releases/my-api-abc123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(response.headers().get("location").unwrap().to_str().unwrap().starts_with("/login"));
}

#[tokio::test]
async fn artifact_detail_non_member_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/unknown-org/projects/my-api/releases/some-slug")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ─── Usage ──────────────────────────────────────────────────────────

#[tokio::test]
async fn usage_page_returns_200() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/usage")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Early Access"));
    assert!(html.contains("testorg"));
}

#[tokio::test]
async fn usage_page_unauthenticated_redirects() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/settings/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(response.headers().get("location").unwrap().to_str().unwrap().starts_with("/login"));
}

#[tokio::test]
async fn usage_page_non_member_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/unknown-org/settings/usage")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ─── Nav & Error rendering ──────────────────────────────────────────

#[tokio::test]
async fn authenticated_pages_show_app_nav() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Sign out"));
    assert!(html.contains("testorg"));
    assert!(!html.contains("Sign in"));
}

#[tokio::test]
async fn error_403_renders_html() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/unknown-org/projects")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Access denied"));
}

// ─── Destinations ────────────────────────────────────────────────────

#[tokio::test]
async fn destinations_page_returns_200() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/destinations")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Destinations"));
}

#[tokio::test]
async fn destinations_page_unauthenticated_redirects() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/destinations")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(response.headers().get("location").unwrap().to_str().unwrap().starts_with("/login"));
}

#[tokio::test]
async fn destinations_page_non_member_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/otherorg/destinations")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn destinations_page_shows_empty_state() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/destinations")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("No environments yet"));
}

// ─── Releases ────────────────────────────────────────────────────────

#[tokio::test]
async fn releases_page_returns_200() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/releases")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Continuous deployment"));
}

#[tokio::test]
async fn releases_page_unauthenticated_redirects() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/releases")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(response.headers().get("location").unwrap().to_str().unwrap().starts_with("/login"));
}

#[tokio::test]
async fn releases_page_non_member_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/otherorg/releases")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn releases_page_shows_empty_state() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_projects_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/releases")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    // Empty state is now rendered client-side by the Svelte component
    assert!(html.contains("release-timeline"));
    assert!(html.contains("org=\"testorg\""));
}

// ─── User profile ──────────────────────────────────────────────────

#[tokio::test]
async fn user_profile_shows_username() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/users/testuser")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("testuser"));
    assert!(html.contains("Member since"));
}

// ─── Triggers (auto-release) ────────────────────────────────────────

#[tokio::test]
async fn triggers_page_returns_200() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/triggers")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Triggers"));
}

#[tokio::test]
async fn triggers_page_shows_existing_triggers() {
    use forage_core::platform::Trigger;

    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_triggers_result: Some(Ok(vec![Trigger {
            id: "t1".into(),
            name: "deploy-main".into(),
            enabled: true,
            branch_pattern: Some("main".into()),
            title_pattern: None,
            author_pattern: None,
            commit_message_pattern: None,
            source_type_pattern: None,
            target_environments: vec!["staging".into()],
            target_destinations: vec![],
            force_release: false,
            use_pipeline: false,
            created_at: "2026-03-08T00:00:00Z".into(),
            updated_at: "2026-03-08T00:00:00Z".into(),
        }])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/triggers")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("deploy-main"));
    assert!(html.contains("staging"));
}

#[tokio::test]
async fn create_trigger_requires_admin() {
    let (state, sessions) = test_state();
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/projects/my-api/triggers")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("csrf_token=test-csrf&name=test-trigger"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_trigger_requires_csrf() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/projects/my-api/triggers")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("csrf_token=wrong-token&name=test-trigger"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_trigger_success_redirects() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/projects/my-api/triggers")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("csrf_token=test-csrf&name=deploy-main&branch_pattern=main&target_environments=staging")
                )
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "/orgs/testorg/projects/my-api/triggers"
    );
}

#[tokio::test]
async fn toggle_trigger_requires_admin() {
    let (state, sessions) = test_state();
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/projects/my-api/triggers/deploy-main/toggle")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("csrf_token=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_trigger_success_redirects() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/projects/my-api/triggers/deploy-main/delete")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("csrf_token=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "/orgs/testorg/projects/my-api/triggers"
    );
}

// ─── Deployment Policies ────────────────────────────────────────────

#[tokio::test]
async fn policies_page_returns_200() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/policies")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Deployment Policies"));
}

#[tokio::test]
async fn create_policy_requires_admin() {
    let (state, sessions) = test_state();
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/projects/my-api/policies")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("csrf_token=test-csrf&name=test-policy&policy_type=soak_time"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_policy_requires_csrf() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/projects/my-api/policies")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("csrf_token=wrong-token&name=test-policy&policy_type=soak_time"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
