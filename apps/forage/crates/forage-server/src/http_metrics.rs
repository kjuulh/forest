use std::time::Instant;

use axum::{extract::MatchedPath, extract::Request, middleware::Next, response::Response};
use opentelemetry::{global, metrics::Histogram, KeyValue};
use std::sync::OnceLock;

const UNMATCHED: &str = "<unmatched>";

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

fn route_of(request: &Request) -> String {
    route_label(
        request
            .extensions()
            .get::<MatchedPath>()
            .map(|m| m.as_str()),
    )
}

fn route_label(matched: Option<&str>) -> String {
    matched.unwrap_or(UNMATCHED).to_owned()
}

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
        let route = route_label(Some("/orgs/{org}/projects/{project}/releases"));
        assert_eq!(route, "/orgs/{org}/projects/{project}/releases");
        assert!(
            !route.contains("understory"),
            "an org would leak into the label"
        );
    }

    #[test]
    fn an_unmatched_request_never_becomes_a_label() {
        assert_eq!(route_label(None), UNMATCHED);
    }

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
