use axum::body::Body;
use axum::http::{Request, StatusCode};
use forage_core::platform::PlatformError;
use forage_core::registry::{
    ComponentDetail, ComponentSearchResult, ComponentSummary, ComponentVersionInfo, ToolShape,
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
        shape: forage_core::registry::ToolShape::Component,
        tool: None,
        methods: vec![],
        upstream_host: String::new(),
    }
}

fn sample_versions() -> Vec<ComponentVersionInfo> {
    vec![
        ComponentVersionInfo {
            version: "1.2.0".into(),
            protocol_version: "1".into(),
            kind: "binary".into(),
            platforms: vec!["linux_amd64".into(), "darwin_arm64".into()],
            created_at: "2026-01-02T00:00:00Z".into(),
        },
        ComponentVersionInfo {
            version: "1.1.0".into(),
            protocol_version: "1".into(),
            kind: "binary".into(),
            platforms: vec!["linux_amd64".into()],
            created_at: "2026-01-01T00:00:00Z".into(),
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
    assert!(html.contains("component"));
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

#[tokio::test]
async fn components_search_anonymous_uses_public_rpc_only() {
    let private = ComponentSummary {
        visibility: "private".into(),
        ..sample_summary()
    };
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        search_components_result: Some(Ok(ComponentSearchResult {
            components: vec![private],
            total_count: 1,
        })),
        search_public_components_result: Some(Ok(ComponentSearchResult {
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
    assert!(
        !html.contains("testorg/deployment"),
        "private component leaked into anonymous /components: {html}",
    );
    assert!(html.contains("No components published yet"));
}

#[tokio::test]
async fn component_detail_anonymous_404_on_private() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_public_component_detail_result: Some(Err(PlatformError::NotFound(
            "not public".into(),
        ))),
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

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn component_detail_sessioned_200_for_private_when_has_access() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_public_component_detail_result: Some(Err(PlatformError::NotFound(
            "not public".into(),
        ))),
        get_component_detail_result: Some(Ok(sample_detail())),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/deployment")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn component_detail_sessioned_404_when_no_access() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_public_component_detail_result: Some(Err(PlatformError::NotFound(
            "not public".into(),
        ))),
        get_component_detail_result: Some(Err(PlatformError::NotFound(
            "no access".into(),
        ))),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/deployment")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn component_version_detail_anonymous_404_on_private() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_public_component_detail_result: Some(Err(PlatformError::NotFound(
            "not public".into(),
        ))),
        get_public_component_manifest_result: Some(Err(PlatformError::NotFound(
            "not public".into(),
        ))),
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

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn component_version_detail_sessioned_200_for_private_when_has_access() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_public_component_detail_result: Some(Err(PlatformError::NotFound(
            "not public".into(),
        ))),
        get_public_component_manifest_result: Some(Err(PlatformError::NotFound(
            "not public".into(),
        ))),
        get_component_detail_result: Some(Ok(sample_detail())),
        get_component_manifest_result: Some(Ok(
            r#"{"name":"deployment","version":"1.1.0"}"#.into(),
        )),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/deployment/1.1.0")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn component_version_detail_sessioned_404_when_no_access() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_public_component_detail_result: Some(Err(PlatformError::NotFound(
            "not public".into(),
        ))),
        get_public_component_manifest_result: Some(Err(PlatformError::NotFound(
            "not public".into(),
        ))),
        get_component_detail_result: Some(Err(PlatformError::NotFound("no access".into()))),
        get_component_manifest_result: Some(Err(PlatformError::NotFound("no access".into()))),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/deployment/1.1.0")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

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
    assert!(html.contains("published"));
    assert!(
        html.contains("2026-01-02T00:00:00Z"),
        "expected the publish timestamp in the release card"
    );
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
    assert!(html.contains("Components"));
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

#[tokio::test]
async fn org_components_renders_install_command() {
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
    assert!(
        html.contains("forest global add testorg"),
        "expected org install command in page; got: {html}"
    );
    assert!(
        html.contains("org-install-cmd"),
        "expected copyable code-block id for install command"
    );
}

#[tokio::test]
async fn org_components_install_command_uses_path_org_not_session_org() {
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
    let cookie = create_test_session_with_orgs(&sessions, &["testorg", "second-org"]).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/second-org/components")
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
    let cmd = extract_pre_text(&html, "org-install-cmd");
    assert_eq!(
        cmd, "forest global add second-org",
        "install command should embed path org, not session org"
    );
}

#[tokio::test]
async fn org_components_install_caption_uses_path_org() {
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
    let cookie = create_test_session_with_orgs(&sessions, &["testorg", "second-org"]).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/orgs/second-org/components")
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
    assert!(
        html.contains(r#"<span class="font-mono">second-org</span>"#),
        "caption span should contain the path org name; got: {html}"
    );
    assert!(
        html.contains("Subscribe to every tool in second-org"),
        "dropdown header should name the path org; got: {html}"
    );
}

#[tokio::test]
async fn org_components_install_button_hidden_when_no_components() {
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
    assert!(
        !html.contains("forest global add testorg"),
        "install button must not render when org has zero components"
    );
}

#[tokio::test]
async fn org_components_install_button_visible_on_filtered_empty_search() {
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
                .uri("/orgs/testorg/components?q=nonexistent")
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
    assert!(
        html.contains("forest global add testorg"),
        "install button should still render on filtered-empty searches"
    );
}

#[tokio::test]
async fn org_components_empty_state_renders_when_no_components() {
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
    assert!(
        html.contains("No components published yet."),
        "empty-state copy must render when org has zero components"
    );
    assert!(
        !html.contains("forest global add testorg"),
        "install button must still be hidden alongside the empty state"
    );
}

#[tokio::test]
async fn org_components_install_uses_native_details_element() {
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
    let summary_marker = "<summary class=\"inline-flex items-center gap-1.5 px-3 py-1.5 bg-green-600";
    let summary_open = html
        .find(summary_marker)
        .expect("install dropdown <summary> with bg-green-600 must exist");
    let summary_close_rel = html[summary_open..]
        .find("</summary>")
        .expect("install <summary> must be closed");
    let summary_slice = &html[summary_open..summary_open + summary_close_rel];
    assert!(
        summary_slice.contains("Install"),
        "install <summary> must contain the visible label \"Install\"; got: {summary_slice}"
    );
    let before_summary = &html[..summary_open];
    let parent_details_open = before_summary
        .rfind("<details")
        .expect("install <summary> must be nested inside <details> for keyboard toggling");
    let between = &before_summary[parent_details_open..];
    assert!(
        !between.contains("</details>"),
        "install <summary> must be inside an *open* <details>, not after a closed one; window: {between}"
    );
}

#[tokio::test]
async fn org_context_nav_points_components_tab_at_org_components() {
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
    assert!(
        html.contains(r#"href="/orgs/testorg/components""#),
        "org-context Components nav tab must link to /orgs/{{org}}/components"
    );
}

fn extract_pre_text(html: &str, id: &str) -> String {
    let needle = format!(r#"<pre id="{id}""#);
    let start = html
        .find(&needle)
        .unwrap_or_else(|| panic!("missing <pre id=\"{id}\"> in rendered html"));
    let after_id = &html[start..];
    let content_start = after_id
        .find('>')
        .unwrap_or_else(|| panic!("malformed <pre> for id {id}"))
        + 1;
    let rest = &after_id[content_start..];
    let content_end = rest
        .find("</pre>")
        .unwrap_or_else(|| panic!("unclosed <pre> for id {id}"));
    rest[..content_end].trim().to_string()
}

#[tokio::test]
async fn project_components_tab_renders_versions() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        list_component_versions_result: Some(Ok(sample_versions())),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
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
    assert!(html.contains("v1.1.0"));
}

#[tokio::test]
async fn components_without_registry_returns_503() {
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

fn sample_tool_summary(shape: ToolShape) -> ComponentSummary {
    ComponentSummary {
        organisation: "testorg".into(),
        name: "forest-hello".into(),
        latest_version: "0.1.0".into(),
        kind: "binary".into(),
        description: "Print a friendly greeting".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-03-01T00:00:00Z".into(),
        version_count: 2,
        contracts: vec![],
        visibility: "public".into(),
        shape,
        tool: Some(forage_core::registry::ToolFacet {
            name: "forest-hello".into(),
            argv_passthrough: true,
            description: "Print a friendly greeting".into(),
        }),
        methods: if shape == ToolShape::Hybrid {
            vec!["greet".into(), "status".into()]
        } else {
            vec![]
        },
        upstream_host: if shape == ToolShape::ToolExternal {
            "github.com".into()
        } else {
            String::new()
        },
    }
}

fn sample_tool_detail(shape: ToolShape) -> ComponentDetail {
    ComponentDetail {
        summary: sample_tool_summary(shape),
        versions: vec![ComponentVersionInfo {
            version: "0.1.0".into(),
            protocol_version: "1".into(),
            kind: "binary".into(),
            platforms: vec!["linux_amd64".into()],
            created_at: "2026-01-01T00:00:00Z".into(),
        }],
        readme: String::new(),
        manifest_json: r#"{"kind":"binary","tool":{"name":"forest-hello","argv_passthrough":true}}"#
            .into(),
        owners: vec![],
    }
}

#[tokio::test]
async fn components_list_shows_tool_shape_badge_for_tool_binary() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        search_components_result: Some(Ok(ComponentSearchResult {
            components: vec![sample_tool_summary(ToolShape::ToolBinary)],
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
    assert!(html.contains("forest-hello"));
    assert!(html.contains(">tool<"));
}

#[tokio::test]
async fn components_list_shows_upstream_host_for_external() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        search_components_result: Some(Ok(ComponentSearchResult {
            components: vec![sample_tool_summary(ToolShape::ToolExternal)],
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

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("github.com"));
    assert!(html.contains(">tool-ext<"));
}

#[tokio::test]
async fn component_detail_renders_install_block_for_tool_binary() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Ok(sample_tool_detail(ToolShape::ToolBinary))),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/forest-hello")
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
    assert!(html.contains("forest global add testorg/forest-hello"));
    assert!(html.contains("forest global add testorg/forest-hello@0.1.0"));
    assert!(html.contains("Argv-passthrough"));
    assert!(html.contains("v0.1.0"));
    assert!(html.contains("latest"));
}

#[tokio::test]
async fn component_detail_no_global_install_for_plain_component() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Ok(sample_tool_detail(ToolShape::Component))),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/forest-hello")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(!html.contains("forest global add"));
    assert!(html.contains("forest components add"));
}

#[tokio::test]
async fn component_detail_hybrid_shows_methods_and_install() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Ok(sample_tool_detail(ToolShape::Hybrid))),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/forest-hello")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("forest global add"));
    assert!(html.contains("Methods"));
    assert!(html.contains("greet"));
    assert!(html.contains("status"));
}

#[tokio::test]
async fn component_detail_external_shows_upstream_section() {
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Ok(sample_tool_detail(ToolShape::ToolExternal))),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/forest-hello")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Upstream"));
    assert!(html.contains("github.com"));
}

#[tokio::test]
async fn component_detail_distribution_table_renders_platforms() {
    let mut detail = sample_tool_detail(ToolShape::ToolBinary);
    detail.manifest_json = r#"{
        "kind": "binary",
        "tool": {"name": "forest-hello", "argv_passthrough": true},
        "platforms": {
            "linux_amd64": {"sha256": "5df1c90d18b8cba88100df635f1914f900ebdf17be6652a6ae17a5833ceec945", "size": 438888}
        }
    }"#.into();
    let registry = MockRegistryClient::with_behavior(MockRegistryBehavior {
        get_component_detail_result: Some(Ok(detail)),
        ..Default::default()
    });
    let (state, sessions) =
        test_state_with_registry(MockForestClient::new(), MockPlatformClient::new(), registry);
    let app = crate::build_router(state);
    let cookie = create_test_session(&sessions).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/components/testorg/forest-hello")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Distribution"));
    assert!(html.contains("linux_amd64"));
    assert!(html.contains("5df1c9"));
    assert!(html.contains("ec945"));
    assert!(html.contains("428.6 KB"));
    assert!(html.contains("View raw manifest JSON"));
}

