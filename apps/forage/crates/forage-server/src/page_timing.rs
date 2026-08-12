//! Server-side page timing for the footer.
//!
//! Measures the whole request — routing, handler, database work, template
//! render — rather than just the template, because "why was that page slow"
//! is almost never the template.
//!
//! # Why body substitution
//!
//! The number is only known *after* the handler has produced the body, so a
//! template global cannot carry it. The alternatives were worse:
//!
//! - Threading a start `Instant` through every handler's context: invasive,
//!   and easy to forget on a new page.
//! - A `Server-Timing` header read back by JS: no body rewriting, but Firefox
//!   and Safari do not expose `serverTiming` in Navigation Timing, so the
//!   number would silently vanish for a chunk of users.
//!
//! So the base template emits [`RENDER_MS_PLACEHOLDER`] and this middleware
//! swaps in the elapsed milliseconds on the way out.
//!
//! # Ordering
//!
//! This layer must sit *inside* the compression layer, or it would be handed
//! an already-compressed body and the placeholder would never match. In axum
//! the last `.layer()` applied is the outermost, so this is registered before
//! `CompressionLayer`. Only `text/html` responses are buffered; everything
//! else (static files, JSON, streams) passes straight through untouched.

use std::time::Instant;

use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderValue},
    middleware::Next,
    response::Response,
};

/// Token the base template emits where the server timing should appear.
///
/// Deliberately ugly so it cannot collide with page content, and so an
/// unreplaced one is obvious rather than looking like real output.
pub const RENDER_MS_PLACEHOLDER: &str = "__FOREST_RENDER_MS__";

/// Cap on the HTML we will buffer to perform the substitution. Pages are tens
/// of KiB; anything far larger is not a page we should be holding in memory,
/// so it passes through with the placeholder left alone.
const MAX_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Time the request and substitute the elapsed milliseconds into the HTML.
pub async fn layer(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let response = next.run(request).await;
    let elapsed = started.elapsed();

    if !is_html(&response) {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_BUFFER_BYTES).await {
        Ok(b) => b,
        // Body too large or already consumed — nothing sensible to rewrite.
        // The page still renders; the footer just shows the raw placeholder,
        // which is a cosmetic failure rather than a broken response.
        Err(_) => return Response::from_parts(parts, Body::empty()),
    };

    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    if !text.contains(RENDER_MS_PLACEHOLDER) {
        return Response::from_parts(parts, Body::from(bytes));
    }

    let rendered = text.replace(RENDER_MS_PLACEHOLDER, &format_ms(elapsed));

    // Content-Length is now wrong. Drop it and let the body length speak for
    // itself; leaving a stale value would truncate or hang the response.
    parts.headers.remove(header::CONTENT_LENGTH);
    // Expose the same number as a standard header, so it is available to
    // curl and to browsers whose devtools surface Server-Timing.
    if let Ok(value) = HeaderValue::from_str(&format!("total;dur={:.1}", elapsed.as_secs_f64() * 1000.0))
    {
        parts.headers.insert("server-timing", value);
    }

    Response::from_parts(parts, Body::from(rendered))
}

/// True when the response is an HTML document we should rewrite.
fn is_html(response: &Response) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"))
}

/// Render a duration the way the footer wants it: whole milliseconds once we
/// are past 1ms, sub-millisecond precision below that so a fast page does not
/// read as a suspicious "0ms".
fn format_ms(elapsed: std::time::Duration) -> String {
    let ms = elapsed.as_secs_f64() * 1000.0;
    if ms >= 10.0 {
        format!("{ms:.0}ms")
    } else if ms >= 1.0 {
        format!("{ms:.1}ms")
    } else {
        format!("{ms:.2}ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use std::time::Duration;
    use tower::ServiceExt;

    fn html(body: &'static str) -> Response {
        Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(body))
            .unwrap()
    }

    async fn run(app: Router, path: &str) -> (axum::http::HeaderMap, String) {
        let response = app
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (headers, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[test]
    fn format_keeps_precision_for_fast_pages() {
        // A sub-millisecond page must not read as "0ms" — that looks broken.
        assert_eq!(format_ms(Duration::from_micros(400)), "0.40ms");
        assert_eq!(format_ms(Duration::from_micros(2_500)), "2.5ms");
        assert_eq!(format_ms(Duration::from_millis(12)), "12ms");
        assert_eq!(format_ms(Duration::from_millis(1_500)), "1500ms");
    }

    #[tokio::test]
    async fn substitutes_the_placeholder_in_html() {
        let app = Router::new()
            .route(
                "/",
                get(|| async { html("<footer>page __FOREST_RENDER_MS__</footer>") }),
            )
            .layer(axum::middleware::from_fn(layer));

        let (headers, body) = run(app, "/").await;
        assert!(
            !body.contains(RENDER_MS_PLACEHOLDER),
            "placeholder should have been replaced: {body}"
        );
        assert!(body.contains("<footer>page "), "{body}");
        assert!(body.ends_with("ms</footer>"), "{body}");
        assert!(
            headers.contains_key("server-timing"),
            "should also expose the timing as a header"
        );
        // The invariant is that no *stale* length survives: either the
        // header is gone, or it matches the rewritten body exactly.
        if let Some(len) = headers.get(header::CONTENT_LENGTH) {
            let declared: usize = len.to_str().unwrap().parse().unwrap();
            assert_eq!(
                declared,
                body.len(),
                "Content-Length {declared} does not match rewritten body {}",
                body.len()
            );
        }
    }

    #[tokio::test]
    async fn html_without_the_placeholder_is_passed_through_byte_for_byte() {
        let app = Router::new()
            .route("/", get(|| async { html("<p>no footer here</p>") }))
            .layer(axum::middleware::from_fn(layer));
        let (_, body) = run(app, "/").await;
        assert_eq!(body, "<p>no footer here</p>");
    }

    #[tokio::test]
    async fn non_html_responses_are_never_rewritten() {
        // JSON containing the token must survive verbatim — the middleware has
        // no business touching API payloads.
        let app = Router::new()
            .route(
                "/api",
                get(|| async {
                    Response::builder()
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"note":"__FOREST_RENDER_MS__"}"#))
                        .unwrap()
                }),
            )
            .layer(axum::middleware::from_fn(layer));

        let (headers, body) = run(app, "/api").await;
        assert_eq!(body, r#"{"note":"__FOREST_RENDER_MS__"}"#);
        assert!(
            !headers.contains_key("server-timing"),
            "non-HTML should be left entirely alone"
        );
    }

    #[tokio::test]
    async fn measures_the_whole_request_not_just_the_template() {
        let app = Router::new()
            .route(
                "/slow",
                get(|| async {
                    // Stand-in for handler + database work.
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    html("took __FOREST_RENDER_MS__")
                }),
            )
            .layer(axum::middleware::from_fn(layer));

        let (_, body) = run(app, "/slow").await;
        let ms: f64 = body
            .trim_start_matches("took ")
            .trim_end_matches("ms")
            .parse()
            .unwrap_or_else(|e| panic!("unparseable timing in {body:?}: {e}"));
        assert!(
            ms >= 25.0,
            "should include handler time, got {ms}ms from {body:?}"
        );
    }

    #[tokio::test]
    async fn a_404_page_is_timed_too() {
        // The fallback renders base.html.jinja as well, so its footer needs the
        // substitution just as much as a 200 does.
        let app = Router::new()
            .fallback(get(|| async {
                (
                    axum::http::StatusCode::NOT_FOUND,
                    html("missing __FOREST_RENDER_MS__"),
                )
            }))
            .layer(axum::middleware::from_fn(layer));
        let (_, body) = run(app, "/nope").await;
        assert!(!body.contains(RENDER_MS_PLACEHOLDER), "{body}");
    }
}
