use std::path::Path;

use anyhow::Context;
use minijinja::Environment;
use sha2::Digest;

/// Format an ISO 8601 / RFC 3339 timestamp as a human-friendly relative time.
fn timeago(value: &str) -> String {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(&format!("{value}Z")))
        .or_else(|_| {
            // Try parsing "2026-01-01" as a date
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map(|d| {
                    d.and_hms_opt(0, 0, 0)
                        .unwrap()
                        .and_utc()
                        .fixed_offset()
                })
        })
    else {
        return value.to_string();
    };

    let now = chrono::Utc::now();
    let diff = now.signed_duration_since(dt);

    if diff.num_seconds() < 60 {
        "just now".into()
    } else if diff.num_minutes() < 60 {
        let m = diff.num_minutes();
        format!("{m}m ago")
    } else if diff.num_hours() < 24 {
        let h = diff.num_hours();
        format!("{h}h ago")
    } else if diff.num_days() < 30 {
        let d = diff.num_days();
        format!("{d}d ago")
    } else {
        dt.format("%d %b %Y").to_string()
    }
}

/// Format a future ISO 8601 / RFC 3339 timestamp as a relative countdown.
fn timeuntil(value: &str) -> String {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(&format!("{value}Z")))
    else {
        return value.to_string();
    };

    let now = chrono::Utc::now();
    let diff = dt.signed_duration_since(now);

    if diff.num_seconds() <= 0 {
        "now".into()
    } else if diff.num_seconds() < 60 {
        let s = diff.num_seconds();
        format!("in {s}s")
    } else if diff.num_minutes() < 60 {
        let m = diff.num_minutes();
        let s = diff.num_seconds() % 60;
        if s > 0 {
            format!("in {m}m {s}s")
        } else {
            format!("in {m}m")
        }
    } else if diff.num_hours() < 24 {
        let h = diff.num_hours();
        let m = diff.num_minutes() % 60;
        if m > 0 {
            format!("in {h}h {m}m")
        } else {
            format!("in {h}h")
        }
    } else {
        let d = diff.num_days();
        format!("in {d}d")
    }
}

/// Format an ISO 8601 / RFC 3339 timestamp as a full human-readable datetime.
fn datetime(value: &str) -> String {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(&format!("{value}Z")))
        .or_else(|_| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").map(|d| {
                d.and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_utc()
                    .fixed_offset()
            })
        })
    else {
        return value.to_string();
    };

    dt.format("%d %b %Y %H:%M:%S UTC").to_string()
}

#[derive(Clone)]
pub struct TemplateEngine {
    env: Environment<'static>,
}

impl TemplateEngine {
    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            anyhow::bail!("templates directory not found: {}", path.display());
        }

        let mut env = Environment::new();
        env.set_loader(minijinja::path_loader(path));
        env.add_filter("timeago", |v: String| -> String { timeago(&v) });
        env.add_filter("timeuntil", |v: String| -> String { timeuntil(&v) });
        env.add_filter("datetime", |v: String| -> String { datetime(&v) });
        env.add_filter("urlencode", |v: String| -> String {
            urlencoding::encode(&v).into_owned()
        });
        // Compact sha256 chip — `5df1c9…ec945`. Used in the Platforms table
        // and anywhere a long content-address would dominate the row.
        env.add_filter("short_sha", |v: String| -> String {
            crate::manifest_view::ManifestView::short_sha(&v)
        });
        // Human-readable byte count for manifest sizes (e.g. "438.0 KB").
        env.add_filter("human_size", |v: u64| -> String {
            crate::manifest_view::ManifestView::human_size(v)
        });

        // Default asset hash for tests/dev — overridden in production by compute_asset_hashes
        env.add_global("css_hash", "dev");

        Ok(Self { env })
    }

    pub fn new() -> anyhow::Result<Self> {
        let mut engine = Self::from_path(Path::new("templates"))?;
        engine.compute_asset_hashes();
        Ok(engine)
    }

    /// Compute content hashes for static assets and inject them as globals.
    /// Templates can use `{{ css_hash }}` for cache-busting query strings.
    fn compute_asset_hashes(&mut self) {
        let css_hash = match std::fs::read("static/css/style.css") {
            Ok(bytes) => {
                let digest = sha2::Sha256::digest(&bytes);
                format!("{:x}", digest)[..12].to_string()
            }
            Err(_) => "dev".to_string(),
        };
        self.env.add_global("css_hash", css_hash);
    }

    pub fn render(&self, template: &str, ctx: minijinja::Value) -> anyhow::Result<String> {
        let tmpl = self
            .env
            .get_template(template)
            .with_context(|| format!("template not found: {template}"))?;
        tmpl.render(ctx)
            .with_context(|| format!("failed to render template: {template}"))
    }
}
