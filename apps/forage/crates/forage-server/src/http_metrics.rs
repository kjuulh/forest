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
}
