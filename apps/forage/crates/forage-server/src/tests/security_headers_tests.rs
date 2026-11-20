use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use tower::ServiceExt;

use crate::security_headers::{ENFORCED_POLICY, REPORT_ONLY_POLICY};
use crate::test_support::*;
use crate::{build_maintenance_router, build_router};

async fn send(app: axum::Router, request: Request<Body>) -> Response<Body> {
    app.oneshot(request).await.unwrap()
}

async fn get(uri: &str) -> Response<Body> {
    let (state, _sessions) = test_state_with_magic_link();
    send(
        build_router(state),
        Request::builder().uri(uri).body(Body::empty()).unwrap(),
    )
    .await
}

async fn post_report(content_type: &str, body: Vec<u8>) -> StatusCode {
    let (state, _sessions) = test_state();
    send(
        build_router(state),
        Request::builder()
            .method("POST")
            .uri("/csp-report")
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .unwrap(),
    )
    .await
    .status()
}

fn header_of(response: &Response<Body>, name: header::HeaderName) -> &str {
    response
        .headers()
        .get(&name)
        .unwrap_or_else(|| panic!("missing {name}"))
        .to_str()
        .unwrap()
}

fn assert_secured(response: &Response<Body>) {
    assert_eq!(
        header_of(response, header::CONTENT_SECURITY_POLICY),
        ENFORCED_POLICY
    );
    assert_eq!(
        header_of(response, header::CONTENT_SECURITY_POLICY_REPORT_ONLY),
        REPORT_ONLY_POLICY
    );
    assert_eq!(
        header_of(response, header::X_CONTENT_TYPE_OPTIONS),
        "nosniff"
    );
    assert_eq!(header_of(response, header::X_FRAME_OPTIONS), "DENY");
}

#[tokio::test]
async fn pages_errors_and_static_files_carry_the_security_headers() {
    for uri in ["/login", "/no-such-page", "/static/css/style.css"] {
        let response = get(uri).await;
        assert_secured(&response);
        assert_eq!(
            header_of(&response, header::REFERRER_POLICY),
            "strict-origin-when-cross-origin",
            "{uri}"
        );
    }
}

#[tokio::test]
async fn the_maintenance_page_carries_the_security_headers() {
    let (state, _sessions) = test_state();
    let response = send(
        build_maintenance_router(state.templates),
        Request::builder().uri("/").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_secured(&response);
}

#[test]
fn the_enforced_policy_forbids_framing_and_base_and_plugins() {
    for directive in [
        "frame-ancestors 'none'",
        "base-uri 'none'",
        "object-src 'none'",
    ] {
        assert!(ENFORCED_POLICY.contains(directive), "{directive}");
    }
}

#[test]
fn the_report_only_policy_allows_no_inline_or_foreign_script() {
    assert!(REPORT_ONLY_POLICY.starts_with("default-src 'none';"));
    assert!(REPORT_ONLY_POLICY.contains("script-src 'self';"));
    assert!(!REPORT_ONLY_POLICY.contains("unsafe-inline"));
    assert!(!REPORT_ONLY_POLICY.contains("unsafe-eval"));
    assert!(REPORT_ONLY_POLICY.contains("form-action 'self'"));
    assert!(REPORT_ONLY_POLICY.ends_with("report-uri /csp-report"));
}

#[tokio::test]
async fn csp_reports_are_accepted() {
    let legacy = serde_json::json!({"csp-report": {
        "document-uri": "https://forest.understory.sh/orgs/acme?token=secret",
        "violated-directive": "style-src-elem",
        "effective-directive": "style-src-elem",
        "blocked-uri": "inline",
    }});
    assert_eq!(
        post_report("application/csp-report", legacy.to_string().into_bytes()).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn malformed_and_oversized_csp_reports_are_rejected() {
    for body in [&b"not json"[..], &br#"[{"body":{}}]"#[..]] {
        assert_eq!(
            post_report("application/csp-report", body.to_vec()).await,
            StatusCode::BAD_REQUEST
        );
    }
    let huge = format!(
        "{{\"csp-report\":{{\"blocked-uri\":\"{}\"}}}}",
        "a".repeat(32 * 1024)
    );
    assert_eq!(
        post_report("application/csp-report", huge.into_bytes()).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}
