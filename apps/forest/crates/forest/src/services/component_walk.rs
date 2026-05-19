//! Component file walker for `forest components publish`.
//!
//! Pure core. No gRPC, no async, no global state. Given a component
//! root and a `WalkConfig`, returns the deterministic, sorted list of
//! files to upload, plus a sibling list of skipped paths with reasons
//! so callers (and tests) can introspect why something was dropped.
//!
//! Precedence rules (from spec 021):
//! 1. **Default excludes** — non-overridable safety rails (`.git/`,
//!    `target/`, `node_modules/`, `cue.mod/pkg/`, `.env*`, etc).
//! 2. **`paths.include` allowlist** — when `Some`, only paths matching
//!    one of the globs are eligible.
//! 3. **`.forestignore`** — patterns subtract from whatever the
//!    allowlist (or default) admitted.
//!
//! Defaults always win: a `.forestignore` of `!target/dist` does **not**
//! re-include the default-excluded `target/`.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

/// 10 MiB. Operator-overridable via `FOREST_PUBLISH_MAX_FILE_BYTES`.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// 50 MiB. Operator-overridable via `FOREST_PUBLISH_MAX_TOTAL_BYTES`.
pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 50 * 1024 * 1024;

/// Directory names that are always excluded, regardless of config.
const DEFAULT_EXCLUDE_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".idea",
    ".vscode",
];

/// Filename patterns that are always excluded, regardless of config.
/// Matched against the basename only.
const DEFAULT_EXCLUDE_FILE_GLOBS: &[&str] = &[
    ".forestignore",
    ".DS_Store",
    ".env",
    ".env.*",
    "*.swp",
    "*.swo",
    "*~",
];

/// Path-prefix patterns (rel_path-rooted) that are always excluded.
/// `cue.mod/pkg/**` is vendored cue deps, re-vendored at consume time
/// — never publish them.
const DEFAULT_EXCLUDE_PATH_GLOBS: &[&str] = &["cue.mod/pkg/**"];

#[derive(Debug, Clone)]
pub struct WalkConfig {
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    /// `forest.component.paths.include` globs. `None` ⇒ "include all".
    pub allowlist: Option<Vec<String>>,
    /// `.forestignore` patterns (gitignore-style globs, parsed
    /// line-by-line by the caller).
    pub forestignore: Vec<String>,
    /// The compiled binary path; uploaded separately as a typed
    /// binary, not as a generic file.
    pub binary_path: Option<PathBuf>,
}

impl Default for WalkConfig {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            allowlist: None,
            forestignore: Vec::new(),
            binary_path: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkEntry {
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    DefaultExclude(&'static str),
    Forestignore,
    NotInAllowlist,
    Symlink,
    BinaryArtifact,
}

#[derive(Debug, thiserror::Error)]
pub enum WalkError {
    #[error("file {path} exceeds per-file cap of {cap} bytes (size: {size})")]
    FileTooLarge { path: String, size: u64, cap: u64 },
    #[error("total component size exceeds cap of {cap} bytes (already walked: {total})")]
    TotalTooLarge { total: u64, cap: u64 },
    #[error("path {0:?} contains '..' or is absolute; refusing to publish")]
    UnsafePath(String),
    #[error("invalid glob pattern {pattern:?}: {source}")]
    InvalidGlob {
        pattern: String,
        #[source]
        source: globset::Error,
    },
    #[error("io error walking component tree: {0}")]
    Io(#[from] std::io::Error),
    #[error("walk error: {0}")]
    Walk(String),
}

impl From<walkdir::Error> for WalkError {
    fn from(e: walkdir::Error) -> Self {
        WalkError::Walk(e.to_string())
    }
}

#[derive(Debug, Default)]
pub struct WalkResult {
    pub include: Vec<WalkEntry>,
    pub skipped: Vec<(String, SkipReason)>,
}

pub fn component_walk(root: &Path, config: &WalkConfig) -> Result<WalkResult, WalkError> {
    let default_basename_globs = build_glob_set(DEFAULT_EXCLUDE_FILE_GLOBS, "default-file")?;
    let default_path_globs = build_glob_set(DEFAULT_EXCLUDE_PATH_GLOBS, "default-path")?;
    let forestignore_globs =
        build_glob_set(&config.forestignore.iter().map(String::as_str).collect::<Vec<_>>(), "forestignore")?;
    let allowlist_globs = match &config.allowlist {
        Some(patterns) => Some(build_glob_set(
            &patterns.iter().map(String::as_str).collect::<Vec<_>>(),
            "allowlist",
        )?),
        None => None,
    };

    let canonical_binary = config
        .binary_path
        .as_ref()
        .and_then(|p| std::fs::canonicalize(p).ok());

    let mut result = WalkResult::default();
    let mut total_bytes: u64 = 0;

    let walker = walkdir::WalkDir::new(root).follow_links(false);

    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_file() && !entry.file_type().is_symlink() {
            continue;
        }

        let abs_path = entry.path();
        let Ok(rel) = abs_path.strip_prefix(root) else {
            continue;
        };
        let rel_path = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");

        // Path-traversal refusal.
        if rel_path.is_empty()
            || rel_path.starts_with('/')
            || rel_path.split('/').any(|seg| seg == "..")
        {
            return Err(WalkError::UnsafePath(rel_path));
        }

        // Default excludes (non-overridable). Check first so they
        // short-circuit the rest.
        if let Some(reason) = matches_default_exclude(
            &rel_path,
            &default_basename_globs,
            &default_path_globs,
        ) {
            result.skipped.push((rel_path, SkipReason::DefaultExclude(reason)));
            continue;
        }

        // Symlinks: never followed, never published.
        if entry.file_type().is_symlink() {
            result.skipped.push((rel_path, SkipReason::Symlink));
            continue;
        }

        // The compiled binary is shipped separately; skip it here.
        if let Some(ref bin) = canonical_binary
            && let Ok(canonical_entry) = std::fs::canonicalize(abs_path)
            && canonical_entry == *bin
        {
            result.skipped.push((rel_path, SkipReason::BinaryArtifact));
            continue;
        }

        // Allowlist: only files matching one of the include globs are
        // eligible. `.forestignore` then subtracts from that set.
        if let Some(ref allow) = allowlist_globs
            && !allow.is_match(&rel_path)
        {
            result.skipped.push((rel_path, SkipReason::NotInAllowlist));
            continue;
        }

        // .forestignore subtracts. Default excludes already ran first,
        // so `!target/dist` cannot re-include `target/`.
        if forestignore_globs.is_match(&rel_path) {
            result.skipped.push((rel_path, SkipReason::Forestignore));
            continue;
        }

        // Size enforcement.
        let metadata = entry.metadata()?;
        let size = metadata.len();
        if size > config.max_file_bytes {
            return Err(WalkError::FileTooLarge {
                path: rel_path,
                size,
                cap: config.max_file_bytes,
            });
        }
        total_bytes = total_bytes.saturating_add(size);
        if total_bytes > config.max_total_bytes {
            return Err(WalkError::TotalTooLarge {
                total: total_bytes,
                cap: config.max_total_bytes,
            });
        }

        result.include.push(WalkEntry {
            rel_path,
            abs_path: abs_path.to_path_buf(),
            size,
        });
    }

    result.include.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(result)
}

fn matches_default_exclude(
    rel_path: &str,
    basename_globs: &GlobSet,
    path_globs: &GlobSet,
) -> Option<&'static str> {
    // Any path component matching a default-excluded directory.
    for seg in rel_path.split('/') {
        for d in DEFAULT_EXCLUDE_DIRS {
            if *d == seg {
                return Some(*d);
            }
        }
    }

    if path_globs.is_match(rel_path) {
        return Some("path-glob");
    }

    if let Some(basename) = rel_path.rsplit_once('/').map(|(_, b)| b).or(Some(rel_path))
        && basename_globs.is_match(basename)
    {
        return Some("file-glob");
    }

    None
}

fn build_glob_set(patterns: &[&str], context: &'static str) -> Result<GlobSet, WalkError> {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        let glob = Glob::new(p).map_err(|e| WalkError::InvalidGlob {
            pattern: format!("[{context}] {p}"),
            source: e,
        })?;
        builder.add(glob);
    }
    builder.build().map_err(|e| WalkError::InvalidGlob {
        pattern: format!("[{context}] (set)"),
        source: e,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn make_tree(files: &[(&str, &[u8])]) -> TempDir {
        let tmp = TempDir::new().unwrap();
        for (rel, content) in files {
            let abs = tmp.path().join(rel);
            if let Some(parent) = abs.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&abs, content).unwrap();
        }
        tmp
    }

    fn rel_paths(result: &WalkResult) -> Vec<&str> {
        result.include.iter().map(|e| e.rel_path.as_str()).collect()
    }

    #[test]
    fn empty_dir_returns_empty_include() {
        let tmp = TempDir::new().unwrap();
        let result = component_walk(tmp.path(), &WalkConfig::default()).unwrap();
        assert!(result.include.is_empty());
    }

    #[test]
    fn forest_cue_is_included() {
        let tmp = make_tree(&[("forest.cue", b"package x")]);
        let result = component_walk(tmp.path(), &WalkConfig::default()).unwrap();
        assert_eq!(rel_paths(&result), vec!["forest.cue"]);
    }

    #[test]
    fn deep_template_paths_are_included_with_forward_slashes() {
        let tmp = make_tree(&[
            ("forest.cue", b"package x"),
            ("templates/deployment/forest/terraform@1/main.tf", b"resource"),
            ("templates/deployment/forest/terraform@1/data.tf", b"data"),
            ("README.md", b"hi"),
        ]);
        let result = component_walk(tmp.path(), &WalkConfig::default()).unwrap();
        assert_eq!(
            rel_paths(&result),
            vec![
                "README.md",
                "forest.cue",
                "templates/deployment/forest/terraform@1/data.tf",
                "templates/deployment/forest/terraform@1/main.tf",
            ]
        );
    }

    #[test]
    fn ordering_is_deterministic() {
        let tmp = make_tree(&[
            ("z.cue", b"z"),
            ("a/b.cue", b"b"),
            ("a/a.cue", b"a"),
            ("README.md", b"r"),
        ]);
        let r1 = component_walk(tmp.path(), &WalkConfig::default()).unwrap();
        let r2 = component_walk(tmp.path(), &WalkConfig::default()).unwrap();
        assert_eq!(rel_paths(&r1), rel_paths(&r2));
        assert_eq!(rel_paths(&r1), vec!["README.md", "a/a.cue", "a/b.cue", "z.cue"]);
    }

    #[test]
    fn default_exclude_target_dir() {
        let tmp = make_tree(&[
            ("forest.cue", b"x"),
            ("target/release/junk", b"binary"),
            ("target/.rustc_info.json", b"{}"),
        ]);
        let result = component_walk(tmp.path(), &WalkConfig::default()).unwrap();
        assert_eq!(rel_paths(&result), vec!["forest.cue"]);
        assert!(
            result
                .skipped
                .iter()
                .all(|(_, r)| matches!(r, SkipReason::DefaultExclude(_))),
            "all skipped entries should be DefaultExclude, got {:?}",
            result.skipped
        );
    }

    #[test]
    fn default_exclude_node_modules_and_git() {
        let tmp = make_tree(&[
            ("forest.cue", b"x"),
            (".git/HEAD", b"ref: foo"),
            ("node_modules/pkg/index.js", b"console.log(1)"),
        ]);
        let result = component_walk(tmp.path(), &WalkConfig::default()).unwrap();
        assert_eq!(rel_paths(&result), vec!["forest.cue"]);
    }

    #[test]
    fn default_exclude_dotfiles() {
        let tmp = make_tree(&[
            ("forest.cue", b"x"),
            (".env", b"SECRET=1"),
            (".env.local", b"LOCAL=1"),
            (".DS_Store", b"\0\0"),
            ("a/.env", b"X"),
            ("foo.swp", b"swap"),
            ("foo.txt~", b"backup"),
            (".forestignore", b"target/"),
        ]);
        let result = component_walk(tmp.path(), &WalkConfig::default()).unwrap();
        assert_eq!(rel_paths(&result), vec!["forest.cue"]);
    }

    #[test]
    fn default_exclude_cue_mod_pkg_but_not_cue_mod_module() {
        let tmp = make_tree(&[
            ("forest.cue", b"x"),
            ("cue.mod/module.cue", b"module: \"x\""),
            ("cue.mod/pkg/forest.sh/forest/sdk@v0/spec.cue", b"vendored"),
        ]);
        let result = component_walk(tmp.path(), &WalkConfig::default()).unwrap();
        assert_eq!(
            rel_paths(&result),
            vec!["cue.mod/module.cue", "forest.cue"],
            "cue.mod/module.cue stays; cue.mod/pkg/** is excluded"
        );
    }

    #[test]
    fn forestignore_subtracts_from_default_includes() {
        let tmp = make_tree(&[
            ("forest.cue", b"x"),
            ("dist/build.tar", b"binary"),
            ("dist/info.json", b"{}"),
            ("README.md", b"hi"),
        ]);
        let cfg = WalkConfig {
            forestignore: vec!["dist/**".into()],
            ..Default::default()
        };
        let result = component_walk(tmp.path(), &cfg).unwrap();
        assert_eq!(rel_paths(&result), vec!["README.md", "forest.cue"]);
        assert_eq!(
            result
                .skipped
                .iter()
                .filter(|(_, r)| *r == SkipReason::Forestignore)
                .count(),
            2,
        );
    }

    #[test]
    fn forestignore_negation_cannot_re_include_default_excludes() {
        // !target/dist is meaningless against the default exclude;
        // the forestignore parser would normally treat it as a
        // re-include, but we apply default excludes *first*. The
        // file is gone before forestignore even gets a vote.
        let tmp = make_tree(&[
            ("forest.cue", b"x"),
            ("target/dist/keepme", b"data"),
        ]);
        let cfg = WalkConfig {
            forestignore: vec!["!target/dist/**".into()],
            ..Default::default()
        };
        let result = component_walk(tmp.path(), &cfg).unwrap();
        assert_eq!(rel_paths(&result), vec!["forest.cue"]);
    }

    #[test]
    fn allowlist_only_admits_matching_paths() {
        let tmp = make_tree(&[
            ("forest.cue", b"x"),
            ("templates/deployment/foo/main.tf", b"tf"),
            ("schemas/output.json", b"{}"),
            ("internal/secret.txt", b"shh"),
            ("README.md", b"r"),
        ]);
        let cfg = WalkConfig {
            allowlist: Some(vec![
                "forest.cue".into(),
                "templates/**".into(),
                "schemas/**".into(),
                "README.md".into(),
            ]),
            ..Default::default()
        };
        let result = component_walk(tmp.path(), &cfg).unwrap();
        assert_eq!(
            rel_paths(&result),
            vec![
                "README.md",
                "forest.cue",
                "schemas/output.json",
                "templates/deployment/foo/main.tf",
            ]
        );
        assert!(
            result
                .skipped
                .iter()
                .any(|(p, r)| p == "internal/secret.txt" && *r == SkipReason::NotInAllowlist),
            "internal/secret.txt should be skipped as NotInAllowlist"
        );
    }

    #[test]
    fn allowlist_and_forestignore_compose() {
        let tmp = make_tree(&[
            ("templates/current/main.tf", b"tf"),
            ("templates/old/main.tf", b"tf"),
        ]);
        let cfg = WalkConfig {
            allowlist: Some(vec!["templates/**".into()]),
            forestignore: vec!["templates/old/**".into()],
            ..Default::default()
        };
        let result = component_walk(tmp.path(), &cfg).unwrap();
        assert_eq!(rel_paths(&result), vec!["templates/current/main.tf"]);
    }

    #[test]
    fn per_file_cap_errors_with_path() {
        let tmp = make_tree(&[
            ("forest.cue", b"x"),
            ("big.bin", &[0u8; 1024]),
        ]);
        let cfg = WalkConfig {
            max_file_bytes: 100,
            ..Default::default()
        };
        let err = component_walk(tmp.path(), &cfg).unwrap_err();
        match err {
            WalkError::FileTooLarge { ref path, size, cap } => {
                assert_eq!(path, "big.bin");
                assert_eq!(size, 1024);
                assert_eq!(cap, 100);
            }
            other => panic!("expected FileTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn total_cap_errors() {
        let tmp = make_tree(&[
            ("a.bin", &[0u8; 60]),
            ("b.bin", &[0u8; 60]),
            ("c.bin", &[0u8; 60]),
        ]);
        let cfg = WalkConfig {
            max_file_bytes: 100,
            max_total_bytes: 100,
            ..Default::default()
        };
        let err = component_walk(tmp.path(), &cfg).unwrap_err();
        assert!(matches!(err, WalkError::TotalTooLarge { .. }), "got {err:?}");
    }

    #[test]
    fn binary_path_is_skipped() {
        let tmp = make_tree(&[
            ("forest.cue", b"x"),
            ("ecs-service", b"binary blob"),
        ]);
        let cfg = WalkConfig {
            binary_path: Some(tmp.path().join("ecs-service")),
            ..Default::default()
        };
        let result = component_walk(tmp.path(), &cfg).unwrap();
        assert_eq!(rel_paths(&result), vec!["forest.cue"]);
        assert!(
            result
                .skipped
                .iter()
                .any(|(p, r)| p == "ecs-service" && *r == SkipReason::BinaryArtifact),
        );
    }

    #[test]
    fn symlink_is_skipped_not_followed() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("forest.cue"), b"x").unwrap();
        fs::write(tmp.path().join("real.txt"), b"real").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path().join("real.txt"), tmp.path().join("link.txt")).unwrap();

        let result = component_walk(tmp.path(), &WalkConfig::default()).unwrap();

        // real.txt is a regular file → included.
        // link.txt is a symlink → skipped.
        let included: Vec<&str> = rel_paths(&result);
        assert!(included.contains(&"forest.cue"));
        assert!(included.contains(&"real.txt"));
        assert!(!included.contains(&"link.txt"));

        #[cfg(unix)]
        assert!(
            result
                .skipped
                .iter()
                .any(|(p, r)| p == "link.txt" && *r == SkipReason::Symlink),
            "expected link.txt skipped as Symlink, got {:?}",
            result.skipped,
        );
    }

    #[test]
    fn invalid_glob_in_allowlist_errors() {
        let tmp = make_tree(&[("forest.cue", b"x")]);
        let cfg = WalkConfig {
            allowlist: Some(vec!["[".into()]), // unbalanced
            ..Default::default()
        };
        let err = component_walk(tmp.path(), &cfg).unwrap_err();
        assert!(matches!(err, WalkError::InvalidGlob { .. }), "got {err:?}");
    }
}
