use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use tower::ServiceExt;

use crate::build_router;
use crate::test_support::*;

const SHELL: &str = forage_ui::layouts::shell::shell::SEDX_LAYOUT;

struct Page {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

async fn send(app: axum::Router, request: Request<Body>) -> Page {
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    Page {
        status,
        headers,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
    }
}

async fn signed_in_get(sedx_layout: Option<&str>) -> Page {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let mut request = Request::builder()
        .uri("/settings/account")
        .header("cookie", &cookie);
    if let Some(id) = sedx_layout {
        request = request.header("sedx-layout", id);
    }
    send(build_router(state), request.body(Body::empty()).unwrap()).await
}

fn varies_on_sedx_layout(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::VARY)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| {
            v.split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("sedx-layout"))
        })
}

#[tokio::test]
async fn a_normal_request_gets_the_whole_page_marked_with_the_shell_layout() {
    let page = signed_in_get(None).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.body
            .contains(&format!(r#"<html lang="en" sedx-layout="{SHELL}">"#)),
        "{}",
        page.body
    );
    assert!(page.body.contains(r#"<script src="/static/sedx.js?v="#));
    assert!(page.body.contains(r#"<meta charset="UTF-8">"#));
    assert!(varies_on_sedx_layout(&page.headers), "{:?}", page.headers);
    assert!(page.headers.get("sedx-layout").is_none());
}

#[tokio::test]
async fn a_sedx_request_for_the_shown_shell_gets_only_its_regions() {
    let full = signed_in_get(None).await;
    let partial = signed_in_get(Some(SHELL)).await;
    assert_eq!(partial.status, StatusCode::OK);
    assert_eq!(
        partial
            .headers
            .get("sedx-layout")
            .unwrap()
            .to_str()
            .unwrap(),
        SHELL
    );
    assert!(varies_on_sedx_layout(&partial.headers));
    assert!(partial.body.contains(r#"sedx-region="0""#));
    assert!(partial.body.contains(r#"sedx-region="1""#));
    assert!(
        partial
            .body
            .contains("<title>Account Settings - Forest</title>")
    );
    assert!(
        partial
            .body
            .contains(r#"action="/settings/account/username""#)
    );
    assert!(partial.body.contains(r#"action="/logout" sedx-ignore"#));
    assert!(
        full.body.contains("\n<head sedx-region=\"0\">"),
        "{}",
        full.body
    );
    assert!(
        !partial.body.contains("\n<head sedx-region=\"0\">"),
        "the static text between the shell's regions is not resent: {}",
        partial.body
    );
    assert!(partial.body.len() < full.body.len());
}

#[tokio::test]
async fn a_sedx_request_naming_another_layout_gets_the_whole_page() {
    let page = signed_in_get(Some("0000000000000000")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.headers.get("sedx-layout").is_none());
    assert!(
        page.body
            .contains(&format!(r#"<html lang="en" sedx-layout="{SHELL}">"#))
    );
}

#[tokio::test]
async fn a_redirect_after_a_sedx_post_becomes_sedx_location_and_keeps_the_cookie() {
    let (state, sessions) = test_state();
    let cookie = create_test_session(&sessions).await;
    let page = send(
        build_router(state),
        Request::builder()
            .method("POST")
            .uri("/logout")
            .header("cookie", &cookie)
            .header("sedx-layout", SHELL)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("_csrf=test-csrf"))
            .unwrap(),
    )
    .await;
    assert_eq!(page.status, StatusCode::NO_CONTENT);
    assert_eq!(page.headers.get("sedx-location").unwrap(), "/");
    assert!(page.headers.get(header::LOCATION).is_none());
    assert!(page.headers.get(header::SET_COOKIE).is_some());
    assert_eq!(sessions.session_count(), 0);
}

#[tokio::test]
async fn sedx_js_is_served_with_a_long_cache_outside_the_middleware() {
    let page = send(
        test_app(),
        Request::builder()
            .uri("/static/sedx.js?v=abc")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(page.body, sedx::JS);
    assert!(
        page.headers
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/javascript")
    );
    assert_eq!(
        page.headers.get(header::CACHE_CONTROL).unwrap(),
        "public, max-age=31536000, immutable"
    );
    assert!(!varies_on_sedx_layout(&page.headers));
}
