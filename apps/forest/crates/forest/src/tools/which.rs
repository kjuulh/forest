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
    std::env::split_paths(&path_var).any(|dir| is_executable(&dir.join(name)))
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
        assert!(missing.iter().any(|t| t.name == "definitely-not-a-real-binary-xyz"));
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

        let orig = std::env::var_os("PATH");
        // SAFETY: not thread-safe with concurrent PATH readers; no other test
        // in this module reads PATH at the same time.
        unsafe {
            std::env::set_var("PATH", &dir);
        }
        assert!(binary_on_path("fakebin"));
        assert!(!binary_on_path("fakelib"));
        assert!(!binary_on_path("does-not-exist-anywhere"));
        unsafe {
            match orig {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
