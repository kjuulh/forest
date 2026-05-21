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
    // Shape badge ("component") replaces the old kind badge in list views;
    // the per-shape vocabulary is documented in `tool_shape_badge`.
    assert!(html.contains("component"));
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

// ── Org-scoped install button (spec 011) ──────────────────────
//
// `/orgs/{org}/components` surfaces `forest global add <org>` so users
// can subscribe their workstation to the entire org catalogue in one
// command. The button mirrors the per-tool install dropdown on
// `project_detail.html.jinja` but renders a *single* command (no version
// pin — `forest global add` doesn't accept a version at org scope).

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
    // The session belongs to the default test org, but the URL targets a
    // different org slug. The install command must reflect the URL slug.
    // This catches the regression where someone wires the button to a
    // session variable instead of the route param.
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
    // Tightened (post-review A3): extract the literal text content of
    // <pre id="org-install-cmd"> and assert exact equality, rather than
    // a substring search against the whole document. The mock returns
    // sample_summary() (organisation = "testorg"), which renders
    // "testorg" in unrelated hrefs; a substring-only assertion is one
    // careless edit away from a false negative.
    let cmd = extract_pre_text(&html, "org-install-cmd");
    assert_eq!(
        cmd, "forest global add second-org",
        "install command should embed path org, not session org"
    );
}

#[tokio::test]
async fn org_components_install_caption_uses_path_org() {
    // Companion to the command assertion above (post-review C1). The
    // dropdown caption ("Subscribes your workstation to <org>'s catalogue
    // …") is the second place `org_name` lands in the markup. A
    // future regression that hardcodes "your org" here would slip past
    // a test that only checks the install command. We assert the
    // caption span carries the path org slug.
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
    // Caption template:
    //   …Subscribes your workstation to <span class="font-mono">{{ org_name }}</span>…
    // A successful render contains the exact span markup with the path org.
    assert!(
        html.contains(r#"<span class="font-mono">second-org</span>"#),
        "caption span should contain the path org name; got: {html}"
    );
    // And the dropdown header line ("Subscribe to every tool in <org>")
    // is the other org-naming spot inside the panel — same logic.
    assert!(
        html.contains("Subscribe to every tool in second-org"),
        "dropdown header should name the path org; got: {html}"
    );
}

#[tokio::test]
async fn org_components_install_button_hidden_when_no_components() {
    // True-empty catalogue (no components, no query). Hide the install
    // button — there is nothing for `forest global add` to install yet,
    // and the empty-state CTA already tells the user to publish first.
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
    // The catalogue is non-empty in principle, but the user's query
    // filtered everything out. Still show the install button — the org
    // *does* publish things, the user just narrowed too far.
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
    // Sister of `_hidden_when_no_components` (post-review C4): the
    // earlier test only asserts the install button does *not* render;
    // it can't detect a regression that eats the entire `{% else %}`
    // branch and leaves the page blank where the empty state used to
    // live. This test pins the empty-state copy explicitly.
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
    // Markup-structure guard (post-review C2-lite). The spec promises
    // keyboard accessibility via native <details>/<summary>; a
    // regression to <div onclick="..."> would pass every other test
    // (the strings would still render) but break Enter/Space toggling
    // for keyboard users. We don't have a browser driver, but we
    // *can* assert the element is structurally what we promised.
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
    // base.html.jinja has its own <details>/<summary> elements for
    // nav dropdowns, so we can't just grep for the first <summary>.
    // Anchor on the install button's distinctive bg-green-600 class —
    // that's unique to the install summary.
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
    // And the parent must be <details> — keyboard a11y guarantee.
    // Stronger than "rfind('<details') is Some": that would false-pass
    // if a nav <details> opened+closed earlier and the green summary
    // landed inside a sibling <div>. Find the latest <details opening
    // before the summary and assert no </details> intervenes — i.e.,
    // that <details> is genuinely still open when the summary appears.
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

/// Extract the inner text of the first `<pre id="…">…</pre>` block,
/// trimmed. Used by spec-011 tests to assert exact equality on the
/// rendered install command rather than substring-matching against
/// the whole document (which is brittle when unrelated content
/// happens to contain the org slug).
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

// ── Project-level Components tab ──────────────────────────────
//
// `/orgs/{org}/projects/{project}/components` is the project's full
// Components tab — sibling of Releases. The Overview's sidebar shows a
// top-3 summary; this page lists every version.

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
    // Lists the versions from the mock fixture.
    assert!(html.contains("v1.2.0"));
    assert!(html.contains("v1.1.0"));
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

// ─── Merged surface (specs/features/007) ─────────────────────────────────
//
// Tools and components share one set of routes: `/components`, `/orgs/{org}/
// components`, and `/components/{org}/{name}`. The detail page renders
// shape-aware sections — Install + Invocation/Methods/Upstream + pretty
// manifest — only when the artefact carries a tool facet.

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
        }],
        readme: String::new(),
        manifest_json: r#"{"kind":"binary","tool":{"name":"forest-hello","argv_passthrough":true}}"#
            .into(),
        owners: vec![],
    }
}

/// A tool_binary artefact appears in the unified components list with the
/// "tool" shape badge — no separate Tools tab to discover from.
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
    // Shape badge `tool` is the text emitted by `tool_shape_badge` for
    // ToolBinary.
    assert!(html.contains(">tool<"));
}

/// External-tool list rows surface the upstream host chip on the list view.
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

/// Tool shapes get the global-install copy block on the detail page.
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
    // MiniJinja HTML-escapes `/` in plain-text contexts; the on-the-wire
    // form is `testorg&#x2f;forest-hello`. The browser textContent (used by
    // the clipboard button) sees the unescaped `/`.
    assert!(html.contains("forest global add testorg&#x2f;forest-hello"));
    assert!(html.contains("forest global add testorg&#x2f;forest-hello@0.1.0"));
    // Invocation block for argv-passthrough binaries.
    assert!(html.contains("Argv-passthrough"));
    // Releases card with the latest pill.
    assert!(html.contains("v0.1.0"));
    assert!(html.contains("latest"));
}

/// Plain component shape does NOT get the global-install copy block — those
/// surfaces only exist for artefacts with a tool facet.
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
    // No global-install block — components are added to a project via
    // `forest components add` (kept as a sidebar action), never installed
    // globally.
    assert!(!html.contains("forest global add"));
    assert!(html.contains("forest components add"));
}

/// Hybrid shape gets BOTH the Methods list AND the global-install block
/// (it's the canonical "two surfaces" case).
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
    // Install block (because hybrid has a tool facet).
    assert!(html.contains("forest global add"));
    // Methods listed (because hybrid also exposes methods).
    assert!(html.contains("Methods"));
    assert!(html.contains("greet"));
    assert!(html.contains("status"));
}

/// External-tool detail surfaces the upstream host in its own section.
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

/// Distribution section renders the manifest's `platforms` map as a native
/// table (sha + human-readable size) — not raw JSON.
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
    // The structured table is present.
    assert!(html.contains("Distribution"));
    assert!(html.contains("linux_amd64"));
    // Short sha chip — the `short_sha` filter renders prefix + ellipsis + suffix.
    assert!(html.contains("5df1c9"));
    assert!(html.contains("ec945"));
    // Human-readable size, not raw bytes.
    assert!(html.contains("428.6 KB"));
    // Raw JSON disclosure remains as a fallback for power users.
    assert!(html.contains("View raw manifest JSON"));
}

