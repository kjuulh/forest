use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

async fn release_page(platform: MockPlatformClient, path: &str) -> (StatusCode, String) {
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri(path)
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

fn main_element(html: &str) -> String {
    let start = html.find("<main").unwrap();
    let end = html.find("</main>").unwrap() + "</main>".len();
    html[start..end].to_string()
}

#[tokio::test]
async fn project_releases_mounts_the_timeline_and_primes_its_data() {
    let (status, html) = release_page(
        MockPlatformClient::new(),
        "/orgs/testorg/projects/my-api/releases",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"<release-timeline id="rt-testorg-my-api-releases" sedx-preserve org="testorg" project="my-api" csrf="test-csrf" username="testuser" role=""#), "{html}");
    assert!(
        html.contains(r#"data-timeline-url="/api/orgs/testorg/projects/my-api/timeline""#),
        "{html}"
    );
    assert!(
        html.contains("/static/js/components/forage-components.js?v="),
        "{html}"
    );
    assert!(
        html.contains(r#"<a href="/orgs/testorg/projects/my-api/releases" aria-current="page""#),
        "the Releases tab is current"
    );
    assert!(
        html.contains("<title>Releases - my-api - testorg - Forest</title>"),
        "{html}"
    );
    assert!(!html.contains(r#"graph="true""#), "{html}");
    assert!(!html.contains("data-include-graph"), "{html}");
    assert!(!html.contains("/static/js/release-graph.js"), "{html}");
    assert!(!html.contains("<script>"), "no inline scripts: {html}");
    let main = regex::Regex::new(r"\?v=[0-9a-z]+")
        .unwrap()
        .replace_all(&main_element(&html), "?v=HASH")
        .into_owned();
    insta::assert_snapshot!(main);

    let (status, html) = release_page(
        MockPlatformClient::new(),
        "/orgs/testorg/projects/my-api/graph",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("<title>Graph - my-api - testorg - Forest</title>"),
        "{html}"
    );
    assert!(
        html.contains(r#"<h1 class="text-2xl font-bold">Graph</h1>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<a href="/orgs/testorg/projects/my-api/graph" aria-current="page""#),
        "{html}"
    );
    assert!(
        html.contains(r#"<release-timeline id="rt-testorg-my-api-graph" sedx-preserve"#),
        "{html}"
    );
    assert!(html.contains(r#"graph="true""#), "{html}");
    assert!(html.contains(r#"data-include-graph="true""#), "{html}");
    assert!(
        html.contains(r#"data-timeline-url="/api/orgs/testorg/projects/my-api/timeline""#),
        "{html}"
    );
    assert!(html.contains("/static/js/release-graph.js?v="), "{html}");

    let (status, html) = release_page(MockPlatformClient::new(), "/orgs/testorg/releases").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(r#"data-timeline-url="/api/orgs/testorg/timeline""#),
        "{html}"
    );
    assert!(
        html.contains(r#"<release-timeline id="rt-testorg-releases" sedx-preserve org="testorg""#),
        "{html}"
    );
    assert!(!html.contains(r#"graph="true""#), "{html}");
    assert!(!html.contains("data-include-graph"), "{html}");
    assert!(!html.contains("/static/js/release-graph.js"), "{html}");
}

#[tokio::test]
async fn project_releases_renders_even_when_listing_artifacts_fails() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_artifacts_result: Some(Err(forage_core::platform::PlatformError::Other(
            "down".into(),
        ))),
        ..Default::default()
    });
    let (status, html) = release_page(platform, "/orgs/testorg/projects/my-api/releases").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the page never showed artifacts; the timeline loads its own"
    );
    assert!(html.contains("release-timeline"));
}

#[tokio::test]
async fn project_graph_requires_a_session_and_org_membership() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let app = build_router(state);
    for (path, cookie, status) in [
        (
            "/orgs/testorg/projects/my-api/graph",
            None,
            StatusCode::SEE_OTHER,
        ),
        (
            "/orgs/unknown-org/projects/my-api/graph",
            Some(cookie.as_str()),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let mut request = Request::builder().uri(path);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{path}");
        if cookie.is_none() {
            assert!(
                response.headers()["location"]
                    .to_str()
                    .unwrap()
                    .starts_with("/login")
            );
        }
    }
}
