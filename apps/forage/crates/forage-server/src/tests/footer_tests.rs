use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::build_router;
use crate::page_timing::RENDER_MS_PLACEHOLDER;
use crate::test_support::*;

async fn get_page(uri: &str) -> (StatusCode, String) {
    let (state, _sessions) = test_state_with_magic_link();
    let app = build_router(state);
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn footer_reports_the_server_version_and_a_real_timing() {
    let (status, html) = get_page("/login").await;
    assert_eq!(status, StatusCode::OK);

    assert!(
        !html.contains(RENDER_MS_PLACEHOLDER),
        "the placeholder must never reach a browser"
    );
    let crate_version = env!("CARGO_PKG_VERSION");
    assert!(
        !html.contains(&format!("v{crate_version}")),
        "footer must not claim the unmanaged crate version as a product version"
    );
    assert!(
        html.contains("page ") && html.contains("ms"),
        "footer should carry a server timing"
    );
    assert!(html.contains("&copy; 2026 Forest") || html.contains("© 2026 Forest"));
}

#[tokio::test]
async fn footer_is_present_on_the_404_page_too() {
    let (status, html) = get_page("/definitely-not-a-real-page").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!html.contains(RENDER_MS_PLACEHOLDER), "{html}");
}

#[tokio::test]
async fn unstamped_builds_omit_commit_and_build_time_rather_than_showing_blanks() {
    let (_, html) = get_page("/login").await;
    let has_commit_stamp = std::env::var("FOREST_GIT_SHA").is_ok_and(|v| !v.trim().is_empty());
    if !has_commit_stamp {
        assert!(
            !html.contains("<code class=\"font-mono\"></code>"),
            "should omit the commit segment entirely when unstamped"
        );
        assert!(
            !html.contains("built <time datetime=\"\">"),
            "should omit the build-time segment entirely when unstamped"
        );
    }
}

#[tokio::test]
async fn the_client_load_placeholder_is_present_for_the_script_to_fill() {
    let (_, html) = get_page("/login").await;
    assert!(
        html.contains("id=\"client-load-ms\""),
        "the script needs its target element"
    );
    assert!(
        html.contains("id=\"client-load-time\""),
        "and the wrapper it unhides"
    );
    assert!(html.contains("id=\"client-load-time\" hidden"), "{html}");
}
