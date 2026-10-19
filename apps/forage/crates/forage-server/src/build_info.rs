#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: String,
    pub commit: String,
    pub build_time: String,
}

const COMMIT_VAR: &str = "FOREST_GIT_SHA";
const BUILD_TIME_VAR: &str = "FOREST_BUILD_TIME";

impl BuildInfo {
    pub fn from_env() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: read(COMMIT_VAR),
            build_time: normalize_build_time(&read(BUILD_TIME_VAR)),
        }
    }
}

fn read(key: &str) -> String {
    match std::env::var(key) {
        Ok(v) => {
            let v = v.trim();
            if v.is_empty() || v.starts_with("${") || v == "unknown" {
                String::new()
            } else {
                v.to_string()
            }
        }
        Err(_) => String::new(),
    }
}

fn normalize_build_time(value: &str) -> String {
    let Ok(timestamp) = value.parse::<i64>() else {
        return value.to_string();
    };

    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|timestamp| timestamp.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_always_reports_the_crate_version() {
        let info = BuildInfo::from_env();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.version.is_empty());
    }

    #[test]
    fn blank_and_placeholder_stamps_read_as_absent() {
        for raw in ["", "   ", "${GIT_SHA}", "unknown"] {
            // SAFETY: single-threaded test, restored immediately below.
            unsafe { std::env::set_var("FOREST_BUILD_INFO_TEST", raw) };
            assert_eq!(read("FOREST_BUILD_INFO_TEST"), "", "{raw:?}");
        }
        unsafe { std::env::remove_var("FOREST_BUILD_INFO_TEST") };
    }

    #[test]
    fn a_real_stamp_is_read_and_trimmed() {
        unsafe { std::env::set_var("FOREST_BUILD_INFO_TEST2", "  437c7b1\n") };
        assert_eq!(read("FOREST_BUILD_INFO_TEST2"), "437c7b1");
        unsafe { std::env::remove_var("FOREST_BUILD_INFO_TEST2") };
    }

    #[test]
    fn unix_build_timestamp_is_normalized_to_rfc3339() {
        assert_eq!(normalize_build_time("1700000000"), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn an_rfc3339_build_timestamp_is_kept_as_stamped() {
        assert_eq!(
            normalize_build_time("2026-09-20T01:00:35+02:00"),
            "2026-09-20T01:00:35+02:00"
        );
    }

    #[test]
    fn empty_and_unconvertible_stamps_are_left_alone() {
        assert_eq!(normalize_build_time(""), "");
        assert_eq!(normalize_build_time("yesterday"), "yesterday");
        assert_eq!(
            normalize_build_time(&i64::MAX.to_string()),
            i64::MAX.to_string()
        );
    }

    #[test]
    fn an_unset_variable_is_absent_not_a_panic() {
        assert_eq!(read("FOREST_BUILD_INFO_DEFINITELY_UNSET_XYZ"), "");
    }
}
