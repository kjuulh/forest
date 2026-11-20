use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

async fn project_releases(platform: MockPlatformClient) -> (StatusCode, String) {
    let (state, sessions) = test_state_with(MockForestClient::new(), platform);
    let cookie = create_test_session(&sessions).await;
    let response = build_router(state)
        .oneshot(
            Request::builder()
                .uri("/orgs/testorg/projects/my-api/releases")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

fn main_element(html: &str) -> String {
    let start = html.find("<main").unwrap();
    let end = html.find("</main>").unwrap() + "</main>".len();
    html[start..end].to_string()
}

#[tokio::test]
async fn project_releases_mounts_the_timeline_and_primes_its_data() {
    let (status, html) = project_releases(MockPlatformClient::new()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"<release-timeline id="rt-testorg-my-api" sedx-preserve org="testorg" project="my-api" csrf="test-csrf" username="testuser" role=""#), "{html}");
    assert!(html.contains(r#"data-timeline-url="/api/orgs/testorg/projects/my-api/timeline""#), "{html}");
    assert!(html.contains("/static/js/components/forage-components.js?v="), "{html}");
    assert!(html.contains(r#"<a href="/orgs/testorg/projects/my-api/releases" aria-current="page""#), "the Releases tab is current");
    assert!(!html.contains("<script>"), "no inline scripts: {html}");
    let main = regex::Regex::new(r"\?v=[0-9a-z]+")
        .unwrap()
        .replace_all(&main_element(&html), "?v=HASH")
        .into_owned();
    insta::assert_snapshot!(main);
}

#[tokio::test]
async fn project_releases_renders_even_when_listing_artifacts_fails() {
    let platform = MockPlatformClient::with_behavior(MockPlatformBehavior {
        list_artifacts_result: Some(Err(forage_core::platform::PlatformError::Other("down".into()))),
        ..Default::default()
    });
    let (status, html) = project_releases(platform).await;
    assert_eq!(status, StatusCode::OK, "the page never showed artifacts; the timeline loads its own");
    assert!(html.contains("release-timeline"));
}
