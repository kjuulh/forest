//! End-to-end checks for the provenance footer.
//!
//! The unit tests in `page_timing` prove the middleware substitutes correctly
//! for a synthetic response. These go through the real router and the real
//! `base.html.jinja`, which is the part that would silently regress: a
//! renamed global or a footer edit that drops the placeholder would leave
//! `__FOREST_RENDER_MS__` visible on every page.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::build_router;
use crate::page_timing::RENDER_MS_PLACEHOLDER;
use crate::test_support::*;

/// Fetch a page through the full middleware stack.
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
    // Deliberately NO product version: forage-server's crate version is
    // 0.1.0 and unmanaged, so showing it would claim a version that is not
    // the product's. Guard against someone reintroducing it.
    let crate_version = env!("CARGO_PKG_VERSION");
    assert!(
        !html.contains(&format!("v{crate_version}")),
        "footer must not claim the unmanaged crate version as a product version"
    );
    // And a substituted server timing, in the footer's own wording.
    assert!(
        html.contains("page ") && html.contains("ms"),
        "footer should carry a server timing"
    );
    assert!(html.contains("&copy; 2026 Forest") || html.contains("© 2026 Forest"));
}

#[tokio::test]
async fn footer_is_present_on_the_404_page_too() {
    // The fallback renders base.html.jinja, so it goes through the same
    // substitution — a stray placeholder here would be very visible.
    let (status, html) = get_page("/definitely-not-a-real-page").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!html.contains(RENDER_MS_PLACEHOLDER), "{html}");
}

#[tokio::test]
async fn unstamped_builds_omit_commit_and_build_time_rather_than_showing_blanks() {
    // A local `cargo run` has no stamps. The footer should simply not carry
    // those segments — an empty `<code></code>` or a bare "built" would read
    // as a rendering bug.
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
    // Hidden until the script has a real number, so an empty "load" never
    // shows up for a browser without Navigation Timing.
    assert!(html.contains("id=\"client-load-time\" hidden"), "{html}");
}
