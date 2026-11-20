use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::{
    body::Bytes,
    extract::Request,
    http::{HeaderName, HeaderValue, StatusCode, header},
    middleware::Next,
    response::Response,
};

pub const REPORT_PATH: &str = "/csp-report";

pub const ENFORCED_POLICY: &str = "frame-ancestors 'none'; base-uri 'none'; object-src 'none'";

pub const REPORT_ONLY_POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
img-src 'self' data: https:; font-src 'self'; connect-src 'self'; manifest-src 'self'; \
base-uri 'none'; form-action 'self'; frame-ancestors 'none'; report-uri /csp-report";

const DEFAULTS: [(HeaderName, &str); 5] = [
    (header::CONTENT_SECURITY_POLICY, ENFORCED_POLICY),
    (
        header::CONTENT_SECURITY_POLICY_REPORT_ONLY,
        REPORT_ONLY_POLICY,
    ),
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (header::X_FRAME_OPTIONS, "DENY"),
    (header::REFERRER_POLICY, "strict-origin-when-cross-origin"),
];

pub async fn layer(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    for (name, value) in DEFAULTS {
        if !headers.contains_key(&name) {
            headers.insert(name, HeaderValue::from_static(value));
        }
    }
    response
}

const MAX_FIELD_CHARS: usize = 200;

fn field(report: &serde_json::Value, keys: &[&str]) -> String {
    let value = keys
        .iter()
        .find_map(|k| report.get(*k).and_then(|v| v.as_str()))
        .unwrap_or("");
    let value = value.split(['?', '#']).next().unwrap_or("");
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_FIELD_CHARS)
        .collect()
}

const SEEN_WINDOW: Duration = Duration::from_secs(3600);
const MAX_SEEN_PER_WINDOW: usize = 100;

pub struct SeenViolations {
    window_start: Instant,
    seen: HashSet<String>,
}

impl SeenViolations {
    pub fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            seen: HashSet::new(),
        }
    }

    pub fn first_in_window(&mut self, key: String, now: Instant) -> bool {
        if now.duration_since(self.window_start) >= SEEN_WINDOW {
            *self = Self::new(now);
        }
        self.seen.len() < MAX_SEEN_PER_WINDOW && self.seen.insert(key)
    }
}

static SEEN: LazyLock<Mutex<SeenViolations>> =
    LazyLock::new(|| Mutex::new(SeenViolations::new(Instant::now())));

pub async fn report(body: Bytes) -> StatusCode {
    let Ok(parsed) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    let Some(r) = parsed.get("csp-report") else {
        return StatusCode::BAD_REQUEST;
    };
    let directive = field(r, &["effective-directive", "violated-directive"]);
    let blocked = field(r, &["blocked-uri"]);
    let source = field(r, &["source-file"]);
    let key = format!("{directive} {blocked} {source}");
    let first = SEEN
        .lock()
        .map(|mut seen| seen.first_in_window(key, Instant::now()))
        .unwrap_or(false);
    if first {
        tracing::warn!(
            target: "csp",
            directive = %directive,
            blocked = %blocked,
            source = %source,
            document = %field(r, &["document-uri"]),
            disposition = %field(r, &["disposition"]),
            "content security policy violation"
        );
    }
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{SeenViolations, field};

    #[test]
    fn a_violation_is_logged_once_per_window_and_distinct_ones_are_capped() {
        let start = Instant::now();
        let mut seen = SeenViolations::new(start);
        assert!(seen.first_in_window("style-src-elem inline".into(), start));
        assert!(!seen.first_in_window("style-src-elem inline".into(), start));
        for i in 1..100 {
            assert!(seen.first_in_window(format!("img-src {i}"), start));
        }
        assert!(!seen.first_in_window("img-src one-too-many".into(), start));
        let later = start + Duration::from_secs(3600);
        assert!(seen.first_in_window("style-src-elem inline".into(), later));
    }

    #[test]
    fn report_fields_drop_query_strings_and_control_characters() {
        let report = serde_json::json!({
            "document-uri": "https://forest.understory.sh/orgs/acme?token=secret#x",
            "blocked-uri": "inline\nforged log line",
        });
        assert_eq!(
            field(&report, &["document-uri"]),
            "https://forest.understory.sh/orgs/acme"
        );
        assert_eq!(field(&report, &["blocked-uri"]), "inlineforged log line");
        assert_eq!(field(&report, &["missing"]), "");
    }
}
