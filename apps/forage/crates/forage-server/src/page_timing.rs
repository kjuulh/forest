use std::time::Instant;

use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderValue},
    middleware::Next,
    response::Response,
};

pub const RENDER_MS_PLACEHOLDER: &str = "__FOREST_RENDER_MS__";

const MAX_BUFFER_BYTES: usize = 4 * 1024 * 1024;

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
        Err(_) => return Response::from_parts(parts, Body::empty()),
    };

    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    if !text.contains(RENDER_MS_PLACEHOLDER) {
        return Response::from_parts(parts, Body::from(bytes));
    }

    let rendered = text.replace(RENDER_MS_PLACEHOLDER, &format_ms(elapsed));

    parts.headers.remove(header::CONTENT_LENGTH);
    if let Ok(value) = HeaderValue::from_str(&format!("total;dur={:.1}", elapsed.as_secs_f64() * 1000.0))
    {
        parts.headers.insert("server-timing", value);
    }

    Response::from_parts(parts, Body::from(rendered))
}

fn is_html(response: &Response) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"))
}

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
