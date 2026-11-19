use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::{
    Artifact, ArtifactContext, ArtifactDestination, ArtifactRef, ArtifactSource, PlatformError,
};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

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
async fn dashboard_recent_activity_orders_across_projects_not_per_project() {
    fn artifact(slug: &str, title: &str, created_at: &str) -> Artifact {
        Artifact {
            artifact_id: slug.into(),
            slug: slug.into(),
            context: ArtifactContext {
                title: title.into(),
                description: None,
                web: None,
                pr: None,
            },
            source: None,
            git_ref: None,
            destinations: vec![],
            created_at: created_at.into(),
        }
    }

    let stale: Vec<Artifact> = (1..=12)
        .map(|i| {
            artifact(
                &format!("stale-{i:02}"),
                &format!("Stale {i:02}"),
                &format!("2026-08-{:02}T10:00:00+00:00", 21 - i),
            )
        })
        .collect();
    let fresh = vec![artifact(
        "fresh-1",
        "Fresh deploy",
        "2026-09-03T09:26:29+00:00",
    )];

    let mut by_project = std::collections::HashMap::new();
    by_project.insert("stale-project".to_string(), stale);
    by_project.insert("fresh-project".to_string(), fresh);

    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_projects_result: Some(Ok(vec![
            "stale-project".into(),
            "filler-2".into(),
            "filler-3".into(),
            "filler-4".into(),
            "filler-5".into(),
            "fresh-project".into(),
        ])),
        list_artifacts_by_project: Some(by_project),
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

    let fresh_at = html
        .find("Fresh deploy")
        .expect("newest release across projects should appear in recent activity");

    let stale_at = html.find("Stale 01").expect("stale releases should appear");
    assert!(
        fresh_at < stale_at,
        "newest release should sort ahead of older ones"
    );

    assert!(html.contains("Stale 09"), "should keep the 9 newest stale");
    assert!(!html.contains("Stale 10"), "should cap at 10 items total");
    assert!(!html.contains("Stale 12"), "oldest should be dropped");
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
async fn create_org_duplicate_shows_actionable_error() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        create_organisation_result: Some(Err(PlatformError::AlreadyExists(
            "resource already exists (idx_organisations_name)".into(),
        ))),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session_no_orgs(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=acme&_csrf=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("An organisation named"));
    assert!(html.contains("acme"));
    assert!(html.contains("ask an owner to invite you"));
    assert!(!html.contains("Could not create organisation"));
}

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
    assert!(html.contains("testuser"));
    assert!(!html.contains("Add member"));
}

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
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

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

fn awaiting_plan_fixture(plan: &str) -> MockPlatformBehavior {
    use forage_core::platform::{PipelineRunStageState, ReleaseIntentState};

    MockPlatformBehavior {
        get_artifact_by_slug_result: Some(Ok(Artifact {
            artifact_id: "art-plan".into(),
            slug: "my-api-plan".into(),
            context: ArtifactContext {
                title: "Adopt the configuration".into(),
                description: None,
                web: None,
                pr: None,
            },
            source: None,
            git_ref: None,
            destinations: vec![],
            created_at: "2026-03-07T12:00:00Z".into(),
        })),
        get_release_intent_states_result: Some(Ok(vec![ReleaseIntentState {
            release_intent_id: "intent-plan".into(),
            artifact_id: "art-plan".into(),
            project: "my-api".into(),
            created_at: "2026-03-07T12:00:00Z".into(),
            stages: vec![PipelineRunStageState {
                stage_id: "plan-finance".into(),
                stage_type: "plan".into(),
                status: "AWAITING_APPROVAL".into(),
                environment: Some("finance".into()),
                approval_status: Some("AWAITING_APPROVAL".into()),
                started_at: Some("2026-03-07T12:00:05Z".into()),
                ..Default::default()
            }],
            steps: vec![],
        }])),
        get_plan_output_result: Some(Ok(forage_core::platform::PlanOutput {
            plan_output: plan.into(),
            status: "SUCCEEDED".into(),
            outputs: vec![],
        })),
        ..Default::default()
    }
}

async fn artifact_detail_html(behavior: MockPlatformBehavior) -> String {
    let platform = MockPlatformClient::with_behavior(behavior);
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/releases/my-api-plan")
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
    String::from_utf8(body.to_vec()).unwrap()
}

#[tokio::test]
async fn a_plan_awaiting_approval_is_shown_on_the_page_that_approves_it() {
    let html = artifact_detail_html(awaiting_plan_fixture(
        "impact: 3 participants change, total delta 1200",
    ))
    .await;

    assert!(html.contains("Approve plan"), "the ask should be there");
    assert!(
        html.contains("impact: 3 participants change, total delta 1200"),
        "the plan being approved should be on the page, got: {html}"
    );
}

#[tokio::test]
async fn a_plan_awaiting_approval_is_open_rather_than_collapsed() {
    let html = artifact_detail_html(awaiting_plan_fixture("would change nothing")).await;
    let details = html
        .split("<details data-plan-output")
        .nth(1)
        .expect("the plan should render in a details element of its own");
    let opening_tag = details.split('>').next().unwrap_or_default();
    assert!(
        opening_tag.contains("open"),
        "a plan awaiting approval should be expanded, got: {opening_tag}"
    );
}

#[tokio::test]
async fn a_stage_with_no_plan_output_renders_no_plan() {
    let mut behavior = awaiting_plan_fixture("");
    behavior.get_plan_output_result = Some(Ok(forage_core::platform::PlanOutput {
        plan_output: "   \n  ".into(),
        status: "SUCCEEDED".into(),
        outputs: vec![],
    }));
    let html = artifact_detail_html(behavior).await;
    assert!(
        !html.contains("<details data-plan-output"),
        "blank plan output should render nothing, got: {html}"
    );
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

#[tokio::test]
async fn reorder_environment_success_redirects() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/destinations/environments/env-prod/order")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf&sort_order=20"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get("location").unwrap(),
        "/orgs/testorg/destinations"
    );
}

#[tokio::test]
async fn reorder_environment_invalid_csrf_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/destinations/environments/env-prod/order")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=wrong-token&sort_order=20"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn reorder_environment_non_admin_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session_member(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/destinations/environments/env-prod/order")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf&sort_order=20"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn reorder_environment_non_member_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/otherorg/destinations/environments/env-prod/order")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf&sort_order=20"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn reorder_environment_unauthenticated_redirects() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/destinations/environments/env-prod/order")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("_csrf=test-csrf&sort_order=20"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(response.headers().get("location").unwrap().to_str().unwrap().starts_with("/login"));
}

#[tokio::test]
async fn destinations_page_renders_drag_handles_for_admin() {
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        MockPlatformClient::with_behavior(MockPlatformBehavior {
            list_environments_result: Some(Ok(vec![
                forage_core::platform::Environment {
                    id: "env-prod".into(),
                    organisation: "testorg".into(),
                    name: "prod".into(),
                    description: None,
                    sort_order: 0,
                    created_at: "2026-03-08T00:00:00Z".into(),
                },
            ])),
            ..Default::default()
        }),
    );
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
    assert!(html.contains("data-env-id=\"env-prod\""), "expected drag-target attribute for admin");
    assert!(html.contains("draggable=\"true\""), "expected draggable cards for admin");
}

#[tokio::test]
async fn destinations_page_no_drag_handles_for_non_admin() {
    let (state, sessions) = test_state_with(
        MockForestClient::new(),
        MockPlatformClient::with_behavior(MockPlatformBehavior {
            list_environments_result: Some(Ok(vec![
                forage_core::platform::Environment {
                    id: "env-prod".into(),
                    organisation: "testorg".into(),
                    name: "prod".into(),
                    description: None,
                    sort_order: 0,
                    created_at: "2026-03-08T00:00:00Z".into(),
                },
            ])),
            ..Default::default()
        }),
    );
    let cookie = create_test_session_member(&sessions).await;
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
    assert!(!html.contains("draggable=\"true\""), "non-admin should not see drag handles");
}

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
    assert!(html.contains("release-timeline"));
    assert!(html.contains("org=\"testorg\""));
}

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

#[tokio::test]
async fn triggers_page_returns_200() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/settings/triggers")
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
                .uri("/orgs/testorg/projects/my-api/settings/triggers")
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
                .uri("/orgs/testorg/projects/my-api/settings/triggers")
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
                .uri("/orgs/testorg/projects/my-api/settings/triggers")
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
                .uri("/orgs/testorg/projects/my-api/settings/triggers")
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
        "/orgs/testorg/projects/my-api/settings/triggers"
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
                .method("POST")
                .uri("/orgs/testorg/projects/my-api/triggers/t1/delete")
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
        "/orgs/testorg/projects/my-api/settings/triggers"
    );
}

#[tokio::test]
async fn policies_page_returns_200() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/settings/policies")
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
    assert!(html.contains("Policies"));
    assert!(html.contains("Gate deployments with soak times"));
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
                .uri("/orgs/testorg/projects/my-api/settings/policies")
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
                .uri("/orgs/testorg/projects/my-api/settings/policies")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("csrf_token=wrong-token&name=test-policy&policy_type=soak_time"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn component_detail_303s_to_project_when_project_exists() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_projects_result: Some(Ok(vec!["my-tool".into()])),
        ..Default::default()
    });
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/my-tool")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(location, "/orgs/testorg/projects/my-tool");
}

#[tokio::test]
async fn component_detail_renders_legacy_page_when_no_project() {
    use forage_core::registry::{ComponentDetail, ComponentSummary, ToolShape};
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_projects_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Ok(ComponentDetail {
            summary: ComponentSummary {
                organisation: "testorg".into(),
                name: "orphan".into(),
                latest_version: "1.0.0".into(),
                kind: "binary".into(),
                description: "orphaned component, no project".into(),
                created_at: String::new(),
                updated_at: String::new(),
                version_count: 1,
                contracts: vec![],
                visibility: "public".into(),
                shape: ToolShape::Component,
                tool: None,
                methods: vec![],
                upstream_host: String::new(),
            },
            versions: vec![],
            readme: String::new(),
            manifest_json: String::new(),
            owners: vec![],
        })),
        ..Default::default()
    });
    let (state, sessions) = test_state_with_registry(MockForestClient::new(), platform, registry);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/orphan")
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
    assert!(html.contains("orphaned component, no project"));
}

#[tokio::test]
async fn deployments_url_303s_to_releases() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/deployments")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(location, "/orgs/testorg/projects/my-api/releases");
}

#[tokio::test]
async fn fresh_project_overview_renders_get_started_panel() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_artifacts_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Err(forage_core::platform::PlatformError::NotFound(
            "no component yet".into(),
        ))),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), platform, registry);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/never-published")
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
    assert!(html.contains("Publish your first version"));
    assert!(!html.contains("<h2 class=\"text-lg font-bold\">Components</h2>"));
    assert!(!html.contains("Recent releases"));
}

#[tokio::test]
async fn project_overview_with_releases_only_shows_swimlane() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Err(forage_core::platform::PlatformError::NotFound(
            "no component yet".into(),
        ))),
        ..Default::default()
    });
    let (state, sessions) = test_state_with_registry(
        MockForestClient::new(),
        MockPlatformClient::new(),
        registry,
    );
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api")
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
    assert!(html.contains("Recent releases"));
    assert!(html.contains("<release-timeline"));
    assert!(!html.contains("Publish your first version"));
}

#[tokio::test]
async fn project_overview_with_components_only_renders_overview() {
    use forage_core::registry::{
        ComponentDetail, ComponentSummary, ComponentVersionInfo, ToolShape,
    };
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_artifacts_result: Some(Ok(vec![])),
        ..Default::default()
    });
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Ok(ComponentDetail {
            summary: ComponentSummary {
                organisation: "testorg".into(),
                name: "my-api".into(),
                latest_version: "1.0.0".into(),
                kind: "binary".into(),
                description: "a component".into(),
                created_at: String::new(),
                updated_at: String::new(),
                version_count: 1,
                contracts: vec![],
                visibility: "public".into(),
                shape: ToolShape::Component,
                tool: None,
                methods: vec![],
                upstream_host: String::new(),
            },
            versions: vec![ComponentVersionInfo {
                version: "1.0.0".into(),
                protocol_version: "1".into(),
                kind: "binary".into(),
                platforms: vec![],
                created_at: "2026-01-01T00:00:00Z".into(),
            }],
            readme: String::new(),
            manifest_json: String::new(),
            owners: vec![],
        })),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), platform, registry);
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api")
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
    assert!(html.contains("v1.0.0"));
    assert!(!html.contains("Publish your first version"));
    assert!(!html.contains("Recent releases"));
}

#[tokio::test]
async fn project_overview_with_releases_and_components_renders_both() {
    use forage_core::registry::{
        ComponentDetail, ComponentSummary, ComponentVersionInfo, ToolShape,
    };
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Ok(ComponentDetail {
            summary: ComponentSummary {
                organisation: "testorg".into(),
                name: "my-api".into(),
                latest_version: "1.0.0".into(),
                kind: "binary".into(),
                description: "a component".into(),
                created_at: String::new(),
                updated_at: String::new(),
                version_count: 1,
                contracts: vec![],
                visibility: "public".into(),
                shape: ToolShape::Component,
                tool: None,
                methods: vec![],
                upstream_host: String::new(),
            },
            versions: vec![ComponentVersionInfo {
                version: "1.0.0".into(),
                protocol_version: "1".into(),
                kind: "binary".into(),
                platforms: vec![],
                created_at: "2026-01-01T00:00:00Z".into(),
            }],
            readme: String::new(),
            manifest_json: String::new(),
            owners: vec![],
        })),
        ..Default::default()
    });
    let (state, sessions) = test_state_with_registry(
        MockForestClient::new(),
        MockPlatformClient::new(),
        registry,
    );
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api")
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
    assert!(html.contains("Recent releases"));
    assert!(html.contains("<release-timeline"));
    assert!(html.contains("v1.0.0"));
    assert!(!html.contains("Publish your first version"));
}

fn extract_nav(html: &str) -> &str {
    let start = html.find("<nav").expect("nav tag");
    let end_rel = html[start..].find("</nav>").expect("end nav tag");
    &html[start..start + end_rel]
}

#[tokio::test]
async fn nav_logo_links_to_dashboard_inside_org() {
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
    let nav = extract_nav(&html);
    assert!(
        nav.contains(r#"href="/dashboard" class="text-lg font-bold tracking-tight shrink-0""#),
        "logo should be a home button linking to /dashboard: {nav}"
    );
    assert!(
        nav.contains(r#"href="/orgs/testorg/projects""#),
        "expected org-scoped link still present in nav, got: {nav}"
    );
}

#[tokio::test]
async fn nav_logo_links_to_dashboard_on_dashboard_page() {
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
    let nav = extract_nav(&html);
    assert!(
        nav.contains(r#"href="/dashboard" class="text-lg font-bold tracking-tight shrink-0""#),
        "logo should link to /dashboard: {nav}"
    );
}

#[tokio::test]
async fn nav_has_no_overview_tab_inside_org() {
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
    let nav = extract_nav(&html);
    let collapsed: String = nav.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        !collapsed.contains(">Overview<"),
        "Overview tab should be absent inside org context: {collapsed}"
    );
}

#[tokio::test]
async fn nav_bell_links_to_org_scoped_notifications() {
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
    let nav = extract_nav(&html);
    assert!(
        nav.contains(r#"href="/orgs/testorg/notifications""#),
        "bell should link to org-scoped notifications: {nav}"
    );
}

#[tokio::test]
async fn org_notifications_route_returns_200() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/notifications")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn org_notifications_non_member_returns_403() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/unknown-org/notifications")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

fn superseded_release_fixture() -> MockPlatformBehavior {
    use forage_core::platform::{
        DeploymentStates, DestinationState, ReleaseIntentState, ReleaseStepState,
    };

    let artifact = |id: &str, slug: &str, title: &str, created: &str| Artifact {
        artifact_id: id.into(),
        slug: slug.into(),
        context: ArtifactContext {
            title: title.into(),
            description: None,
            web: None,
            pr: None,
        },
        source: None,
        git_ref: None,
        destinations: vec![],
        created_at: created.into(),
    };

    let step = |release: &str, status: &str, completed: &str| ReleaseStepState {
        release_id: release.into(),
        stage_id: None,
        destination_name: "prod-cluster".into(),
        environment: "prod".into(),
        status: status.into(),
        queued_at: Some(completed.into()),
        assigned_at: None,
        started_at: Some(completed.into()),
        completed_at: Some(completed.into()),
        error_message: None,
    };

    let intent = |intent_id: &str, artifact_id: &str, created: &str, step: ReleaseStepState| {
        ReleaseIntentState {
            release_intent_id: intent_id.into(),
            artifact_id: artifact_id.into(),
            project: "my-api".into(),
            created_at: created.into(),
            stages: vec![],
            steps: vec![step],
        }
    };

    MockPlatformBehavior {
        list_projects_result: Some(Ok(vec!["my-api".into()])),
        list_artifacts_result: Some(Ok(vec![
            artifact(
                "art-new",
                "my-api-new",
                "the one on prod now",
                "2026-03-08T12:00:00Z",
            ),
            artifact(
                "art-old",
                "my-api-old",
                "deployed, then superseded",
                "2026-03-07T12:00:00Z",
            ),
        ])),
        get_destination_states_result: Some(Ok(DeploymentStates {
            destinations: vec![DestinationState {
                destination_id: "dest-1".into(),
                destination_name: "prod-cluster".into(),
                environment: "prod".into(),
                release_id: Some("rel-new".into()),
                artifact_id: Some("art-new".into()),
                status: Some("SUCCEEDED".into()),
                error_message: None,
                queued_at: Some("2026-03-08T12:00:00Z".into()),
                completed_at: Some("2026-03-08T12:05:00Z".into()),
                queue_position: None,
                started_at: Some("2026-03-08T12:01:00Z".into()),
            }],
        })),
        get_release_intent_states_result: Some(Ok(vec![
            intent(
                "intent-new",
                "art-new",
                "2026-03-08T12:00:00Z",
                step("rel-new", "SUCCEEDED", "2026-03-08T12:05:00Z"),
            ),
            intent(
                "intent-old",
                "art-old",
                "2026-03-07T12:00:00Z",
                step("rel-old", "SUCCEEDED", "2026-03-07T12:05:00Z"),
            ),
        ])),
        ..Default::default()
    }
}

async fn timeline_json(behavior: MockPlatformBehavior) -> serde_json::Value {
    let platform = MockPlatformClient::with_behavior(behavior);
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
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
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn superseded_release_stays_a_release_not_a_hidden_commit() {
    let json = timeline_json(superseded_release_fixture()).await;
    let items = json["timeline"].as_array().unwrap();

    assert!(
        items.iter().all(|i| i["kind"] == "release"),
        "no item should be collapsed as a hidden commit, got: {items:#?}"
    );
    let slugs: Vec<&str> = items
        .iter()
        .map(|i| i["release"]["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, vec!["my-api-new", "my-api-old"]);
}

#[tokio::test]
async fn superseded_release_keeps_its_swimlane_dot() {
    let json = timeline_json(superseded_release_fixture()).await;
    let old = json["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["release"]["slug"] == "my-api-old")
        .expect("superseded release should be on the timeline");

    assert_eq!(old["release"]["dest_envs"], "prod:SUCCEEDED");
    assert_eq!(old["release"]["summary_status"], "SUCCEEDED");
    assert_eq!(old["release"]["destinations"][0]["name"], "prod-cluster");
    assert_eq!(old["release"]["destinations"][0]["status"], "SUCCEEDED");
}

#[tokio::test]
async fn release_without_steps_falls_back_to_destination_states() {
    let mut behavior = superseded_release_fixture();
    behavior.get_release_intent_states_result = Some(Ok(vec![]));
    let json = timeline_json(behavior).await;

    let new = json["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["release"]["slug"] == "my-api-new")
        .expect("current release should be on the timeline");
    assert_eq!(new["release"]["dest_envs"], "prod:SUCCEEDED");
}

#[tokio::test]
async fn is_current_marks_only_the_release_the_destination_still_holds() {
    let json = timeline_json(superseded_release_fixture()).await;
    let items = json["timeline"].as_array().unwrap();

    let current_of = |slug: &str| -> bool {
        items
            .iter()
            .find(|i| i["release"]["slug"] == slug)
            .unwrap_or_else(|| panic!("{slug} should be on the timeline"))["release"]["destinations"]
            [0]["is_current"]
            .as_bool()
            .expect("is_current should be serialised")
    };

    assert!(current_of("my-api-new"), "the current occupant is live");
    assert!(
        !current_of("my-api-old"),
        "the superseded release deployed here, but is no longer live"
    );
}

use std::sync::{Arc, Mutex};

const AWKWARD_NAME: &str = "(feat|feature)/.*-to-platform-dev";

fn awkward_trigger() -> forage_core::platform::Trigger {
    forage_core::platform::Trigger {
        id: "11111111-2222-3333-4444-555555555555".into(),
        name: AWKWARD_NAME.into(),
        enabled: true,
        branch_pattern: Some(AWKWARD_NAME.into()),
        title_pattern: None,
        author_pattern: None,
        commit_message_pattern: None,
        source_type_pattern: None,
        target_environments: vec!["platform-dev".into()],
        target_destinations: vec![],
        force_release: false,
        use_pipeline: false,
        created_at: "2026-03-08T00:00:00Z".into(),
        updated_at: "2026-03-08T00:00:00Z".into(),
    }
}

fn awkward_platform(calls: &Arc<Mutex<Vec<String>>>) -> MockPlatformClient {
    MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_triggers_result: Some(Ok(vec![awkward_trigger()])),
        rule_action_calls: Some(calls.clone()),
        ..Default::default()
    })
}

#[tokio::test]
async fn delete_trigger_with_non_url_safe_name_succeeds_when_addressed_by_id() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (state, sessions) = test_state_with(MockForestClient::new(), awkward_platform(&calls));
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/projects/my-api/triggers/11111111-2222-3333-4444-555555555555/delete")
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
        "/orgs/testorg/projects/my-api/settings/triggers"
    );
    assert_eq!(
        *calls.lock().unwrap(),
        vec![format!("delete_trigger:{AWKWARD_NAME}")]
    );
}

#[tokio::test]
async fn disable_trigger_with_non_url_safe_name_succeeds_when_addressed_by_id() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (state, sessions) = test_state_with(MockForestClient::new(), awkward_platform(&calls));
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/projects/my-api/triggers/11111111-2222-3333-4444-555555555555/toggle")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("csrf_token=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        *calls.lock().unwrap(),
        vec![format!("update_trigger:{AWKWARD_NAME}")]
    );
}

#[tokio::test]
async fn edit_trigger_page_with_non_url_safe_name_loads_when_addressed_by_id() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (state, sessions) = test_state_with(MockForestClient::new(), awkward_platform(&calls));
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/settings/triggers/11111111-2222-3333-4444-555555555555")
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
    assert!(html.contains("(feat|feature)&#x2f;.*-to-platform-dev"));
    assert!(html.contains(
        "action=\"/orgs/testorg/projects/my-api/settings/triggers/11111111-2222-3333-4444-555555555555\""
    ));
}

#[tokio::test]
async fn edit_trigger_page_leaves_unset_patterns_empty() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (state, sessions) = test_state_with(MockForestClient::new(), awkward_platform(&calls));
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/settings/triggers/11111111-2222-3333-4444-555555555555")
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
    for field in [
        "title_pattern",
        "author_pattern",
        "commit_message_pattern",
        "source_type_pattern",
    ] {
        assert!(
            html.contains(&format!("name=\"{field}\" value=\"\"")),
            "{field} should render empty"
        );
    }
    assert!(!html.contains("value=\"none\""));
}

#[tokio::test]
async fn edit_trigger_submit_with_non_url_safe_name_resolves_id_to_name() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (state, sessions) = test_state_with(MockForestClient::new(), awkward_platform(&calls));
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/projects/my-api/settings/triggers/11111111-2222-3333-4444-555555555555")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "csrf_token=test-csrf\
                     &branch_pattern=%28feat%7Cfeature%29%2F.%2A-to-platform-dev\
                     &target_environments=platform-dev",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        *calls.lock().unwrap(),
        vec![format!("update_trigger:{AWKWARD_NAME}")]
    );
}

#[tokio::test]
async fn triggers_page_action_urls_carry_the_id_not_the_name() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (state, sessions) = test_state_with(MockForestClient::new(), awkward_platform(&calls));
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/settings/triggers")
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

    let base =
        "/orgs/testorg/projects/my-api/settings/triggers/11111111-2222-3333-4444-555555555555";
    assert!(html.contains(&format!("{base}/delete")));
    assert!(html.contains(&format!("{base}/toggle")));
    assert!(html.contains(&format!("\"{base}\"")));
    assert!(!html.contains("triggers/(feat"));
    assert!(!html.contains("triggers/%28feat"));
}

#[tokio::test]
async fn delete_trigger_with_unknown_id_is_not_found() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (state, sessions) = test_state_with(MockForestClient::new(), awkward_platform(&calls));
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/orgs/testorg/projects/my-api/triggers/no-such-id/delete")
                .header("cookie", &cookie)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("csrf_token=test-csrf"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(calls.lock().unwrap().is_empty());
}
