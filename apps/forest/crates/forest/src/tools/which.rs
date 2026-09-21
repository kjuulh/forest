//! PATH lookup for external binaries.
//!
//! Lifted out of [`crate::tools::cue`] so the build/publish dispatch (DATA-312)
//! can verify a component's declared required tools up front — instead of
//! letting a missing `cargo`/`go`/`docker` blow up mid-run with a cryptic
//! spawn error.

/// A tool a component requires on PATH, mirrored from CUE `requires.tools`
/// (`#ForestRequiredTool` in the SDK spec). DATA-312.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct RequiredTool {
    /// Binary expected on PATH, e.g. `cargo`, `go`, `docker`.
    pub name: String,
    /// Optional install hint shown when the tool is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Return the subset of `tools` that are NOT on PATH, preserving declared
/// order. An empty result means every required tool is present.
pub fn missing_tools(tools: &[RequiredTool]) -> Vec<RequiredTool> {
    tools
        .iter()
        .filter(|t| !binary_on_path(&t.name))
        .cloned()
        .collect()
}

/// Walk `PATH` and report whether an executable file with the given name
/// exists. No subprocess — sub-millisecond on a warm fs.
pub fn binary_on_path(name: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    binary_in(name, &path_var)
}

/// As [`binary_on_path`], against a search path given explicitly rather than
/// read from the environment. Separate so the lookup can be tested without
/// setting `PATH` for the whole process — `std::env::set_var` is unsound
/// while any other thread reads the environment, and a test binary always has
/// other threads.
fn binary_in(name: &str, search_path: &std::ffi::OsStr) -> bool {
    std::env::split_paths(search_path).any(|dir| is_executable(&dir.join(name)))
}

#[cfg(unix)]
pub fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && (meta.permissions().mode() & 0o111) != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
pub fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tools_reports_only_absent() {
        // A real binary that's effectively always present, plus a fake one.
        let tools = vec![
            RequiredTool {
                name: "definitely-not-a-real-binary-xyz".into(),
                hint: Some("install it".into()),
            },
            RequiredTool {
                name: "sh".into(),
                hint: None,
            },
        ];
        let missing = missing_tools(&tools);
        // `sh` is on PATH on every unix CI box; the fake one never is.
        assert!(
            missing
                .iter()
                .any(|t| t.name == "definitely-not-a-real-binary-xyz")
        );
        assert!(!missing.iter().any(|t| t.name == "sh"));
    }

    #[cfg(unix)]
    #[test]
    fn binary_on_path_finds_executable_and_rejects_non_executable() {
        let dir = std::env::temp_dir().join(format!("forest-which-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("fakebin");
        let nonexe = dir.join("fakelib");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::write(&nonexe, "data").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&nonexe, std::fs::Permissions::from_mode(0o644)).unwrap();

        // Against this directory alone, not the process's PATH. Overwriting
        // PATH here used to make `missing_tools_reports_only_absent` fail
        // whenever the two happened to run at the same moment: it looks up
        // `sh`, which is not in this directory.
        let search_path = dir.as_os_str();
        assert!(binary_in("fakebin", search_path));
        assert!(!binary_in("fakelib", search_path));
        assert!(!binary_in("does-not-exist-anywhere", search_path));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
