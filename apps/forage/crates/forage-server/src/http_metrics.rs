//! How long forage takes to answer, per route, as an OTLP metric.
//!
//! `canopy_otel::init()` already exports traces, logs and metrics, but nothing
//! was producing an HTTP server signal: forage shows up in Dash0 as a
//! `HEADLESS` service with spans and no per-route breakdown, so "which page is
//! slow, and did that change get better or worse" could not be answered from
//! telemetry — only by reading the number the footer prints on one page load.
//!
//! [`page_timing`](crate::page_timing) already measures the whole request for
//! that footer. This records the same measurement as
//! `http.server.request.duration`, the OpenTelemetry HTTP semantic convention,
//! so it lands in Dash0 next to every other service without bespoke dashboards.
//!
//! # Cardinality
//!
//! The route label is the templated path — `/orgs/{org}/projects/{project}`,
//! not `/orgs/understory/projects/forage`. Axum records the former as
//! [`MatchedPath`] during routing, which is why this middleware is registered
//! with `route_layer`: it is the only position that sees it.
//!
//! A request that matched nothing has no template, and its raw path is
//! attacker-controlled, so it is bucketed rather than labelled. One unbounded
//! label is all it takes to make a metrics backend unusable.

use std::time::Instant;

use axum::{extract::MatchedPath, extract::Request, middleware::Next, response::Response};
use opentelemetry::{global, metrics::Histogram, KeyValue};
use std::sync::OnceLock;

/// What an unmatched request is labelled as. Its real path never becomes a
/// label — anyone can invent paths, and each new one is a new time series.
const UNMATCHED: &str = "<unmatched>";

/// Bucket boundaries, in seconds, from the OpenTelemetry HTTP semantic
/// conventions for `http.server.request.duration`.
///
/// These have to be stated. The Rust SDK does not apply the spec's recommended
/// boundaries for a known metric name; it falls back to a default ladder of
/// `[0, 5, 10, 25, … 10000]`, which is scaled for milliseconds. This metric is
/// in seconds, so every real request — forage answers most of them in single-
/// digit milliseconds — landed in the first bucket, and `histogram_quantile`
/// then reported a p95 of 4.75 *seconds* by interpolating across an empty
/// range. The metric shipped, and every percentile drawn from it was fiction.
const DURATION_BUCKETS_SECONDS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

fn duration_histogram() -> &'static Histogram<f64> {
    static HISTOGRAM: OnceLock<Histogram<f64>> = OnceLock::new();
    HISTOGRAM.get_or_init(|| {
        global::meter("forage-server")
            .f64_histogram("http.server.request.duration")
            .with_description(
                "Time to answer an inbound HTTP request: routing, handler, \
                 upstream calls and template render.",
            )
            .with_unit("s")
            .with_boundaries(DURATION_BUCKETS_SECONDS.to_vec())
            .build()
    })
}

/// The templated route for a request, or [`UNMATCHED`].
fn route_of(request: &Request) -> String {
    route_label(
        request
            .extensions()
            .get::<MatchedPath>()
            .map(|m| m.as_str()),
    )
}

/// Split out from [`route_of`] so the decision it makes is testable:
/// `MatchedPath` has no public constructor, so a test cannot build a request
/// that carries one. Whether the router actually supplies it is a wiring
/// question, and the answer is visible in Dash0 the moment this deploys — an
/// `http.route` of `<unmatched>` across the board means the middleware is
/// registered in the wrong position.
fn route_label(matched: Option<&str>) -> String {
    matched.unwrap_or(UNMATCHED).to_owned()
}

/// Record every request against `http.server.request.duration`.
///
/// Must be registered with `route_layer`, not `layer`: `MatchedPath` is put in
/// the request's extensions by the router, so a middleware wrapping the router
/// runs too early to see it and would label everything `<unmatched>`.
pub async fn layer(request: Request, next: Next) -> Response {
    let method = request.method().as_str().to_owned();
    let route = route_of(&request);

    let started = Instant::now();
    let response = next.run(request).await;
    let elapsed = started.elapsed();

    duration_histogram().record(
        elapsed.as_secs_f64(),
        &[
            KeyValue::new("http.request.method", method),
            KeyValue::new("http.route", route),
            KeyValue::new(
                "http.response.status_code",
                i64::from(response.status().as_u16()),
            ),
        ],
    );

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matched_route_is_labelled_by_its_template() {
        // What axum hands us during routing. The label has to be the template,
        // or every org and project in the estate becomes its own time series.
        let route = route_label(Some("/orgs/{org}/projects/{project}/releases"));
        assert_eq!(route, "/orgs/{org}/projects/{project}/releases");
        assert!(
            !route.contains("understory"),
            "an org would leak into the label"
        );
    }

    // Anyone can request any path. Labelling those would let a crawler mint
    // unbounded time series, which is how a metrics bill becomes an incident.
    #[test]
    fn an_unmatched_request_never_becomes_a_label() {
        assert_eq!(route_label(None), UNMATCHED);
    }

    // The boundaries are in seconds because the metric is. The SDK's default
    // ladder tops out at 10000 and starts at 5, which for a seconds-valued
    // metric means everything forage does lands in one bucket — the first
    // version of this shipped that way and reported a p95 of 4.75 seconds for
    // requests it answered in two milliseconds.
    #[test]
    fn the_buckets_are_scaled_for_seconds() {
        let first = DURATION_BUCKETS_SECONDS[0];
        let last = DURATION_BUCKETS_SECONDS[DURATION_BUCKETS_SECONDS.len() - 1];
        assert!(
            first < 0.01,
            "the smallest bucket is {first}s — too coarse to separate a fast page from a slow one"
        );
        assert!(
            last <= 10.0,
            "the largest bucket is {last}s, which suggests millisecond boundaries on a seconds metric"
        );
        assert!(
            DURATION_BUCKETS_SECONDS.windows(2).all(|w| w[0] < w[1]),
            "bucket boundaries must ascend"
        );
    }
}
