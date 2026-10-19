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
            build_time: read(BUILD_TIME_VAR),
        }
    }

    pub fn is_unstamped(&self) -> bool {
        self.commit.is_empty() && self.build_time.is_empty()
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
    fn an_unset_variable_is_absent_not_a_panic() {
        assert_eq!(read("FOREST_BUILD_INFO_DEFINITELY_UNSET_XYZ"), "");
    }

    #[test]
    fn is_unstamped_only_when_both_stamps_are_missing() {
        let bare = BuildInfo {
            version: "0.2.7".into(),
            commit: String::new(),
            build_time: String::new(),
        };
        assert!(bare.is_unstamped());

        let partial = BuildInfo {
            commit: "437c7b1".into(),
            ..bare.clone()
        };
        assert!(!partial.is_unstamped());
    }
}
