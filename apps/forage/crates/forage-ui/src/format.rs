use chrono::{DateTime, FixedOffset, NaiveDate, Utc};

fn parse(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(value)
        .or_else(|_| DateTime::parse_from_rfc3339(&format!("{value}Z")))
        .ok()
}

fn parse_with_date(value: &str) -> Option<DateTime<FixedOffset>> {
    parse(value).or_else(|| {
        NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc().fixed_offset())
    })
}

pub fn timeago(value: &str) -> String {
    timeago_at(value, Utc::now())
}

pub fn timeago_at(value: &str, now: DateTime<Utc>) -> String {
    let Some(dt) = parse_with_date(value) else {
        return value.to_string();
    };
    let diff = now.signed_duration_since(dt);
    if diff.num_seconds() < 60 {
        "just now".into()
    } else if diff.num_minutes() < 60 {
        format!("{}m ago", diff.num_minutes())
    } else if diff.num_hours() < 24 {
        format!("{}h ago", diff.num_hours())
    } else if diff.num_days() < 30 {
        format!("{}d ago", diff.num_days())
    } else {
        dt.format("%d %b %Y").to_string()
    }
}

pub fn timeuntil(value: &str) -> String {
    timeuntil_at(value, Utc::now())
}

pub fn timeuntil_at(value: &str, now: DateTime<Utc>) -> String {
    let Some(dt) = parse(value) else {
        return value.to_string();
    };
    let diff = dt.signed_duration_since(now);
    if diff.num_seconds() <= 0 {
        "now".into()
    } else if diff.num_seconds() < 60 {
        format!("in {}s", diff.num_seconds())
    } else if diff.num_minutes() < 60 {
        let (m, s) = (diff.num_minutes(), diff.num_seconds() % 60);
        if s > 0 {
            format!("in {m}m {s}s")
        } else {
            format!("in {m}m")
        }
    } else if diff.num_hours() < 24 {
        let (h, m) = (diff.num_hours(), diff.num_minutes() % 60);
        if m > 0 {
            format!("in {h}h {m}m")
        } else {
            format!("in {h}h")
        }
    } else {
        format!("in {}d", diff.num_days())
    }
}

pub fn datetime(value: &str) -> String {
    match parse_with_date(value) {
        Some(dt) => dt.format("%d %b %Y %H:%M:%S UTC").to_string(),
        None => value.to_string(),
    }
}

pub fn urlencode(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-19T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn timeago_buckets_by_the_largest_whole_unit() {
        assert_eq!(timeago_at("2026-11-19T11:59:30Z", now()), "just now");
        assert_eq!(timeago_at("2026-11-19T11:15:00Z", now()), "45m ago");
        assert_eq!(timeago_at("2026-11-19T07:00:00Z", now()), "5h ago");
        assert_eq!(timeago_at("2026-11-12T12:00:00Z", now()), "7d ago");
        assert_eq!(timeago_at("2026-01-02T12:00:00Z", now()), "02 Jan 2026");
    }

    #[test]
    fn timeago_accepts_naive_timestamps_and_dates() {
        assert_eq!(timeago_at("2026-11-19T11:15:00", now()), "45m ago");
        assert_eq!(timeago_at("2026-11-12", now()), "7d ago");
    }

    #[test]
    fn unparseable_values_pass_through() {
        assert_eq!(timeago_at("soon", now()), "soon");
        assert_eq!(timeuntil_at("soon", now()), "soon");
        assert_eq!(datetime("soon"), "soon");
    }

    #[test]
    fn timeuntil_counts_down() {
        assert_eq!(timeuntil_at("2026-11-19T11:00:00Z", now()), "now");
        assert_eq!(timeuntil_at("2026-11-19T12:00:30Z", now()), "in 30s");
        assert_eq!(timeuntil_at("2026-11-19T12:05:10Z", now()), "in 5m 10s");
        assert_eq!(timeuntil_at("2026-11-19T14:00:00Z", now()), "in 2h");
        assert_eq!(timeuntil_at("2026-11-21T12:00:00Z", now()), "in 2d");
    }

    #[test]
    fn datetime_is_utc_with_seconds() {
        assert_eq!(datetime("2026-11-19T12:00:05Z"), "19 Nov 2026 12:00:05 UTC");
    }

    #[test]
    fn urlencode_escapes_path_separators() {
        assert_eq!(urlencode("a/b c"), "a%2Fb%20c");
    }
}
