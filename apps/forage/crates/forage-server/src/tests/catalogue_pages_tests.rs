use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use forage_core::registry::{
    ComponentDetail, ComponentSearchResult, ComponentSummary, ComponentVersionInfo, ToolShape,
};
use tower::ServiceExt;

use crate::build_router;
use crate::state::AppState;
use crate::test_support::*;

async fn get(state: AppState, cookie: Option<&str>, uri: &str) -> (StatusCode, String) {
    let mut request = Request::builder().uri(uri);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let response = build_router(state)
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

fn main_snapshot(html: &str) -> String {
    let start = html.find("<main").unwrap();
    let end = html.find("</main>").unwrap() + "</main>".len();
    let main = regex::Regex::new(r"\?v=[0-9a-z]+")
        .unwrap()
        .replace_all(&html[start..end], "?v=HASH")
        .into_owned();
    regex::Regex::new(r"(<time [^>]*>)[^<]*(</time>)")
        .unwrap()
        .replace_all(&main, "${1}AGO${2}")
        .into_owned()
}

fn assert_no_inline_script(html: &str) {
    let script = regex::Regex::new(r"<script(\s[^>]*)?>").unwrap();
    for tag in script.find_iter(html) {
        let tag = tag.as_str();
        assert!(
            tag.contains(" src=") || tag.contains("application/json"),
            "inline script {tag}: {html}"
        );
    }
    assert!(!html.contains("<style"), "no inline styles: {html}");
    for handler in [
        " onclick=",
        " onsubmit=",
        " oninput=",
        " onchange=",
        " style=",
    ] {
        assert!(!html.contains(handler), "inline {handler}: {html}");
    }
}

fn summary(name: &str, shape: ToolShape) -> ComponentSummary {
    ComponentSummary {
        organisation: "testorg".into(),
        name: name.into(),
        latest_version: "1.2.0".into(),
        kind: "binary".into(),
        description: format!("The {name} component"),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-03-01T00:00:00Z".into(),
        version_count: 2,
        contracts: vec!["forest/deployment".into()],
        visibility: "public".into(),
        shape,
        tool: None,
        methods: vec![],
        upstream_host: String::new(),
    }
}

fn versions() -> Vec<ComponentVersionInfo> {
    vec![
        ComponentVersionInfo {
            version: "1.2.0".into(),
            protocol_version: "1".into(),
            kind: "binary".into(),
            platforms: vec!["linux_amd64".into()],
            created_at: "2026-01-02T00:00:00Z".into(),
        },
        ComponentVersionInfo {
            version: "1.1.0".into(),
            protocol_version: String::new(),
            kind: "binary".into(),
            platforms: vec![],
            created_at: String::new(),
        },
    ]
}

fn registry_state(
    behavior: MockRegistryBehavior,
) -> (
    AppState,
    std::sync::Arc<forage_core::session::InMemorySessionStore>,
) {
    test_state_with_registry(
        MockForestClient::new(),
        MockPlatformClient::new(),
        MockRegistryClient::with_behavior(behavior),
    )
}

fn search(components: Vec<ComponentSummary>, total_count: i32) -> MockRegistryBehavior {
    MockRegistryBehavior {
        search_components_result: Some(Ok(ComponentSearchResult {
            components,
            total_count,
        })),
        ..Default::default()
    }
}

#[tokio::test]
async fn the_public_registry_renders_for_visitors_with_its_search_form() {
    let (state, _) = registry_state(search(vec![summary("deployment", ToolShape::Component)], 1));
    let (status, html) = get(state, None, "/components?q=dep").await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"<form method="GET" action="/components""#),
        "{html}"
    );
    assert!(html.contains(r#"name="q" value="dep""#), "{html}");
    assert!(html.contains(r#"<select name="org""#), "{html}");
    assert!(
        html.contains(r#"href="/components/testorg/deployment""#),
        "{html}"
    );
    assert!(html.contains(r#"href="/login""#), "visitor nav: {html}");
    assert!(!html.contains("Sign out"), "{html}");
    assert!(
        !html.contains(">none<"),
        "an unset value leaked as none: {html}"
    );
    assert_no_inline_script(&html);
    insta::assert_snapshot!("components_search", main_snapshot(&html));
}

#[tokio::test]
async fn registry_pagination_keeps_the_query_and_org_filter_encoded() {
    let (state, sessions) = registry_state(search(
        vec![summary("deployment", ToolShape::Component)],
        45,
    ));
    let cookie = create_test_session(&sessions).await;
    let (_, html) = get(
        state,
        Some(&cookie),
        "/components?q=a%20b%26c&org=elsewhere&page=2",
    )
    .await;

    assert!(
        html.contains(r#"href="/components?q=a%20b%26c&amp;org=elsewhere&amp;page=1""#),
        "{html}"
    );
    assert!(
        html.contains(r#"href="/components?q=a%20b%26c&amp;org=elsewhere&amp;page=3""#),
        "{html}"
    );
    assert!(html.contains("Page 2 of 3"), "{html}");
    assert!(
        html.contains(r#"<option value="elsewhere" selected>elsewhere</option>"#),
        "a filter outside my orgs stays selected: {html}"
    );
    assert!(
        html.contains(r#"<option value="testorg">testorg</option>"#),
        "{html}"
    );
    assert!(html.contains("Sign out"), "{html}");
}

#[tokio::test]
async fn an_empty_search_names_the_query_escaped() {
    let (state, _) = registry_state(search(vec![], 0));
    let (_, html) = get(state, None, "/components?q=%3Cb%3E").await;
    assert!(
        html.contains("No components found for &quot;&lt;b&gt;&quot;."),
        "{html}"
    );
    assert!(!html.contains("<b>"), "{html}");
}

fn tool_detail() -> ComponentDetail {
    let mut s = summary("forest-hello", ToolShape::ToolExternal);
    s.upstream_host = "github.com".into();
    ComponentDetail {
        summary: s,
        versions: versions(),
        readme: "# Hello\n\n```rust\nlet x = 1;\n```\n\n<script>alert(1)</script><img src=x onerror=alert(2)>".into(),
        manifest_json: serde_json::json!({
            "kind": "binary",
            "url": "javascript:alert(3)",
            "archive": "tar.gz",
            "archive_sha256": "sha256:5df1c9aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaec945",
            "platforms": {"linux_amd64": {"sha256": "abc", "size": 438886}},
            "include": {"env": {"GREETING": "</script><b>hi</b>"}},
        })
        .to_string(),
        owners: vec!["alice".into()],
    }
}

#[tokio::test]
async fn a_component_page_trusts_only_sanitised_readme_and_our_own_json_markup() {
    let (state, _) = registry_state(MockRegistryBehavior {
        get_component_detail_result: Some(Ok(tool_detail())),
        ..Default::default()
    });
    let (status, html) = get(state, None, "/components/testorg/forest-hello").await;

    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("<h1>Hello</h1>"), "readme rendered: {html}");
    assert!(html.contains(r#"class="language-rust""#), "{html}");
    assert!(!html.contains("alert(1)"), "readme script survived: {html}");
    assert!(!html.contains("onerror"), "{html}");
    assert!(
        html.contains(r#"<span class="json-key">"#),
        "pretty manifest: {html}"
    );
    assert!(
        !html.contains("</script><b>hi</b>"),
        "manifest values stay escaped: {html}"
    );
    assert!(
        html.contains("&lt;/script&gt;&lt;b&gt;hi&lt;/b&gt;"),
        "{html}"
    );
    assert!(
        !html.contains(r#"href="javascript:"#),
        "an upstream URL cannot run script: {html}"
    );
    assert!(html.contains("tool-ext"), "{html}");
    assert!(
        html.contains("forest global add testorg/forest-hello@1.2.0"),
        "{html}"
    );
    assert!(html.contains("/static/js/readme-highlight.js?v="), "{html}");
    assert!(
        html.contains("/static/css/vendor/highlight-github.min.css"),
        "{html}"
    );
    assert!(!html.contains(">none<"), "{html}");
    assert_no_inline_script(&html);
    insta::assert_snapshot!("component_detail_tool", main_snapshot(&html));
}

#[tokio::test]
async fn a_version_page_pins_and_highlights_that_version() {
    let detail = ComponentDetail {
        summary: summary("deployment", ToolShape::Component),
        versions: versions(),
        readme: String::new(),
        manifest_json: String::new(),
        owners: vec![],
    };
    let (state, _) = registry_state(MockRegistryBehavior {
        get_component_detail_result: Some(Ok(detail)),
        ..Default::default()
    });
    let (status, html) = get(state, None, "/components/testorg/deployment/1.1.0").await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("forest components add testorg/deployment@1.1.0"),
        "{html}"
    );
    assert!(
        html.contains(
            r#"<div class="border border-gray-200 rounded-lg p-4 bg-gray-50" id="v1.1.0">"#
        ),
        "{html}"
    );
    assert!(
        html.contains("No README available for this component."),
        "{html}"
    );
    assert!(!html.contains("readme-highlight.js"), "{html}");
    assert!(!html.contains("View raw manifest JSON"), "{html}");
    assert!(
        html.contains(r#"href="/orgs/testorg/components""#),
        "{html}"
    );
    assert_no_inline_script(&html);
    insta::assert_snapshot!("component_version_detail", main_snapshot(&html));
}

#[tokio::test]
async fn the_org_catalogue_offers_install_and_paginates_its_search() {
    let (state, sessions) = registry_state(search(
        vec![summary("deployment", ToolShape::ToolBinary)],
        25,
    ));
    let cookie = create_test_session(&sessions).await;
    let (status, html) = get(state, Some(&cookie), "/orgs/testorg/components?q=dep").await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"<form method="GET" action="/orgs/testorg/components""#),
        "{html}"
    );
    assert!(html.contains(r#"data-copy="org-install-cmd""#), "{html}");
    assert!(
        html.contains(r#"href="/orgs/testorg/components?q=dep&amp;page=2""#),
        "{html}"
    );
    assert_no_inline_script(&html);
    insta::assert_snapshot!("org_components", main_snapshot(&html));
}

#[tokio::test]
async fn the_org_releases_page_primes_the_timeline_and_loads_the_island() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let (status, html) = get(state, Some(&cookie), "/orgs/testorg/releases").await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"<script src="/static/js/timeline-prefetch.js?v="#)
            && html.contains(r#"data-timeline-url="/api/orgs/testorg/timeline""#),
        "the releases page no longer primes the timeline: {html}"
    );
    assert!(
        html.contains(r#"<release-timeline id="rt-testorg" sedx-preserve org="testorg"></release-timeline>"#),
        "{html}"
    );
    assert!(
        html.contains("/static/js/components/forage-components.js?v="),
        "{html}"
    );
    assert_no_inline_script(&html);
    insta::assert_snapshot!("org_releases", main_snapshot(&html));
}

#[test]
fn the_timeline_url_cannot_break_out_of_its_attribute() {
    let page = crate::views::catalogue::OrgReleasesPage {
        chrome: Default::default(),
        org: "evil\"><script>alert(1)</script>".into(),
        prefetch_js: "/static/js/timeline-prefetch.js".into(),
        graph_js: "/static/js/release-graph.js".into(),
    };
    let html = crate::views::pages::releases::render(&page);
    assert!(!html.contains("<script>alert(1)</script>"), "{html}");
}

#[tokio::test]
async fn the_org_releases_page_refuses_non_members() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let (status, html) = get(state, Some(&cookie), "/orgs/other-org/releases").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(!html.contains("release-timeline"), "{html}");
}

#[tokio::test]
async fn the_landing_page_is_for_visitors_and_keeps_its_code_sample_layout() {
    let (state, sessions) = test_state();
    let (status, html) = get(state.clone(), None, "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"href="/login""#), "{html}");
    assert!(html.contains("Your runtime or ours."), "{html}");
    assert!(
        html.contains(
            "<span class=\"text-green-400\">$</span> forest add acme/service-standard@1.4.0\n"
        ),
        "{html}"
    );
    assert_no_inline_script(&html);
    insta::assert_snapshot!("landing", main_snapshot(&html));

    let cookie = create_test_session(&sessions).await;
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri("/")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/dashboard");
}

#[tokio::test]
async fn the_pricing_page_shows_marketing_links_and_signup_calls_to_action() {
    let (state, _) = test_state();
    let (status, html) = get(state, None, "/pricing").await;

    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("<title>Pricing - Forest</title>"), "{html}");
    assert!(html.contains(r#"href="/pricing""#), "{html}");
    assert!(html.contains(r#"href="/components""#), "{html}");
    assert!(
        html.contains(r#"href="mailto:contact@kjuulh.io?subject=Forest%20private%20preview""#),
        "{html}"
    );
    assert_no_inline_script(&html);
    insta::assert_snapshot!("pricing", main_snapshot(&html));
}

#[tokio::test]
async fn the_error_page_renders_for_visitors_and_signed_in_users_alike() {
    let (state, sessions) = test_state();
    let (status, html) = get(state.clone(), None, "/no-such-page").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        html.contains("<title>Page not found - Forest</title>"),
        "{html}"
    );
    assert!(html.contains(r#"href="/login""#), "{html}");
    assert_no_inline_script(&html);
    insta::assert_snapshot!("not_found", main_snapshot(&html));

    let cookie = create_test_session(&sessions).await;
    let (status, html) = get(state, Some(&cookie), "/orgs/other-org/releases").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        html.contains("You don&#39;t have access to this organisation.")
            || html.contains("You don't have access to this organisation."),
        "{html}"
    );
    assert!(
        !html.contains("test-csrf"),
        "the error page carries no session data: {html}"
    );
}

#[tokio::test]
async fn the_maintenance_page_is_standalone_and_its_stylesheet_is_served_in_maintenance_mode() {
    let (state, _) = test_state();
    let app = crate::build_maintenance_router(state.templates);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        html.contains(r#"<link rel="stylesheet" href="/static/css/maintenance.css?v="#),
        "{html}"
    );
    assert!(
        !html.contains("style.css"),
        "no app bundle dependency: {html}"
    );
    assert!(!html.contains("<nav"), "{html}");
    assert_no_inline_script(&html);
    insta::assert_snapshot!(
        "maintenance",
        regex::Regex::new(r"\?v=[0-9a-z]+")
            .unwrap()
            .replace_all(&html, "?v=HASH")
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/static/css/maintenance.css?v=abc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/css")
    );
    let css = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&css).contains(".maintenance-card"));
}
