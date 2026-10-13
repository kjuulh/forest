//! Build provenance for the running binary.
//!
//! Answers "which commit is this server, and when was it built" without
//! shelling into the container. Surfaced over `StatusService.Status` and, for
//! forage, in the page footer.
//!
//! # Why runtime environment rather than compile-time
//!
//! The obvious approach is `option_env!` or a `build.rs`, baking the commit
//! into the binary at compile time. Both are worse here:
//!
//! - The image is built with `context: apps/forage`, so `.git` is not in the
//!   Docker build context — a `build.rs` calling `git rev-parse` finds nothing
//!   and silently degrades to "dev" in exactly the builds we care about.
//! - Passing the commit as a build arg means an `ENV` line *before*
//!   `cargo build`, which changes the build environment on every commit and
//!   invalidates the cached compile.
//!
//! Reading the environment at startup avoids both. The values are baked as
//! `ENV` into the **final** image stage, so the image still self-describes —
//! `docker inspect` shows them — while the expensive builder layers stay
//! cacheable. A local `cargo run` leaves them unset and reports "dev", which
//! is the honest answer for an unstamped build.

/// Provenance of the running binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    /// Crate version of the server binary.
    pub version: String,
    /// Short git commit, or empty when this build was not stamped.
    pub commit: String,
    /// RFC 3339 build timestamp, or empty when not stamped.
    pub build_time: String,
}

/// Environment variable carrying the git commit, set on the image.
const COMMIT_VAR: &str = "FOREST_GIT_SHA";
/// Environment variable carrying an RFC 3339 timestamp or Unix epoch.
const BUILD_TIME_VAR: &str = "FOREST_BUILD_TIME";

impl BuildInfo {
    /// Read provenance from the environment.
    pub fn from_env() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: read(COMMIT_VAR),
            build_time: normalize_build_time(&read(BUILD_TIME_VAR)),
        }
    }
}

/// Read a stamp variable, treating blank/placeholder values as absent.
///
/// CI substitution can leave an empty string or an unexpanded `${VAR}` behind;
/// reporting either verbatim would be worse than reporting nothing, because a
/// caller cannot tell a real commit from a broken pipeline.
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

/// Hold `build_time` to the RFC 3339 contract that status responses and HTML
/// `<time datetime>` attributes expose.
///
/// Some CI systems only offer the build time as Unix seconds (Woodpecker's
/// `CI_PIPELINE_CREATED`, or a `SOURCE_DATE_EPOCH`-style stamp), and passing
/// that through gave the footer a `datetime` browsers cannot parse. An integer
/// is converted; anything else, RFC 3339 included, is left exactly as it was
/// stamped.
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
        // A pipeline that failed to substitute must not look like a real
        // commit — better to report nothing than something unverifiable.
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

    /// Not a timestamp forage can convert, so it is not rewritten into one: an
    /// absent stamp stays absent, and anything else stays visibly what it was.
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
