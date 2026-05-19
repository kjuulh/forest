use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::PlatformError;
use forage_core::registry::{
    ComponentDetail, ComponentSearchResult, ComponentSummary, ComponentVersionInfo,
};
use tower::ServiceExt;

use crate::test_support::*;

fn sample_summary() -> ComponentSummary {
    ComponentSummary {
        organisation: "testorg".into(),
        name: "deployment".into(),
        latest_version: "1.2.0".into(),
        kind: "binary".into(),
        description: "A deployment component".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-03-01T00:00:00Z".into(),
        version_count: 3,
        contracts: vec!["forest/deployment".into()],
        visibility: "public".into(),
    }
}

fn sample_versions() -> Vec<ComponentVersionInfo> {
    vec![
        ComponentVersionInfo {
            version: "1.2.0".into(),
            protocol_version: "1".into(),
            kind: "binary".into(),
            platforms: vec!["linux_amd64".into(), "darwin_arm64".into()],
        },
        ComponentVersionInfo {
            version: "1.1.0".into(),
            protocol_version: "1".into(),
            kind: "binary".into(),
            platforms: vec!["linux_amd64".into()],
        },
    ]
}

fn sample_detail() -> ComponentDetail {
    ComponentDetail {
        summary: sample_summary(),
        versions: sample_versions(),
        readme: "# Deployment Component\n\nA great component.".into(),
        manifest_json: r#"{"name":"deployment","version":"1.2.0"}"#.into(),
        owners: vec!["alice".into()],
    }
}

// ── Public search ──────────────────────────────────────────────

#[tokio::test]
async fn components_search_unauthenticated_returns_200() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        search_components_result: Some(Ok(ComponentSearchResult {
            components: vec![sample_summary()],
            total_count: 1,
        })),
        ..Default::default()
    });
    let (state, _) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components")
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
    assert!(html.contains("testorg/deployment"));
    assert!(html.contains("v1.2.0"));
    assert!(html.contains("binary"));
    // Marketing nav (no user)
    assert!(html.contains("Sign in"));
}

#[tokio::test]
async fn components_search_authenticated_shows_user_nav() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        search_components_result: Some(Ok(ComponentSearchResult {
            components: vec![],
            total_count: 0,
        })),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components")
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
    assert!(html.contains("Sign out"));
}

#[tokio::test]
async fn components_search_empty_shows_placeholder() {
    let registry = MockRegistryClient::new();
    let (state, _) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components")
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
    assert!(html.contains("No components published yet"));
}

#[tokio::test]
async fn components_search_with_query() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        search_components_result: Some(Ok(ComponentSearchResult {
            components: vec![],
            total_count: 0,
        })),
        ..Default::default()
    });
    let (state, _) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components?q=deploy")
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
    assert!(html.contains(r#"value="deploy""#));
    assert!(html.contains("No components found"));
}

// ── Component detail ───────────────────────────────────────────

#[tokio::test]
async fn component_detail_returns_200_with_readme() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Ok(sample_detail())),
        ..Default::default()
    });
    let (state, _) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/deployment")
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
    assert!(html.contains("testorg/deployment"));
    assert!(html.contains("Deployment Component"));
    assert!(html.contains("A great component"));
    assert!(html.contains("v1.2.0"));
    assert!(html.contains("v1.1.0"));
    assert!(html.contains("linux_amd64"));
    assert!(html.contains("alice"));
    assert!(html.contains("forest components add"));
}

#[tokio::test]
async fn component_detail_not_found() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Err(PlatformError::NotFound(
            "not found".into(),
        ))),
        ..Default::default()
    });
    let (state, _) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ── Version-specific detail ────────────────────────────────────

#[tokio::test]
async fn component_version_detail_returns_200() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Ok(sample_detail())),
        get_component_manifest_result: Some(Ok(
            r#"{"name":"deployment","version":"1.1.0"}"#.into(),
        )),
        ..Default::default()
    });
    let (state, _) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/deployment/1.1.0")
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
    assert!(html.contains("testorg/deployment"));
    assert!(html.contains("1.1.0"));
}

// ── Org-scoped component list ──────────────────────────────────

#[tokio::test]
async fn org_components_requires_auth() {
    let registry = MockRegistryClient::new();
    let (state, _) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/components")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Should redirect to login (302) or return 401/403
    assert_ne!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn org_components_returns_200_for_member() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        search_components_result: Some(Ok(ComponentSearchResult {
            components: vec![sample_summary()],
            total_count: 1,
        })),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/components")
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
    assert!(html.contains("deployment"));
    assert!(html.contains("Components")); // active tab
}

#[tokio::test]
async fn org_components_forbidden_for_non_member() {
    let registry = MockRegistryClient::new();
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/other-org/components")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ── Project-level components tab ───────────────────────────────

#[tokio::test]
async fn project_components_returns_200() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        list_component_versions_result: Some(Ok(sample_versions())),
        ..Default::default()
    });
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_projects_result: Some(Ok(vec!["my-component".into()])),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), platform, registry);
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-component/components")
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
    assert!(html.contains("v1.2.0"));
    assert!(html.contains("linux_amd64"));
    assert!(html.contains("forest components add"));
}

// ── No registry configured ─────────────────────────────────────

#[tokio::test]
async fn components_without_registry_returns_503() {
    // Use default test_app which has no registry_client
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/components")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
