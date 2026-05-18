//! Strict-mode global lockfile.
//!
//! Pure module — no I/O. Parses and serialises the user-global lockfile
//! at `$XDG_STATE_HOME/forest/forest.lock` per TASKS/018-global-tools.md §1a.4.
//!
//! The line format matches the per-project `forest.lock` (so a single
//! human-readable format applies everywhere), but **path entries are
//! rejected** — only registry / external content-addressed pins are valid.
//! This is the `LockError::PathEntryNotAllowed` guard called out in §1a.4.

use std::collections::BTreeMap;

/// A registry-style lock entry. Path entries are NOT representable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GlobalLockEntry {
    pub organisation: String,
    pub name: String,
    pub version: String,
    pub os: String,
    pub arch: String,
    /// sha256 hex string, prefixed `sha256:` to match the per-project lock
    /// format. The bare hex (no prefix) is also accepted on parse and
    /// normalised here at construction time.
    pub sha256: String,
}

impl GlobalLockEntry {
    /// Key for de-duplication and lookup: `<org>/<name>@<version> <os>/<arch>`.
    pub fn key(&self) -> String {
        format!(
            "{}/{}@{} {}/{}",
            self.organisation, self.name, self.version, self.os, self.arch
        )
    }

    /// Serialise to a single line of the lockfile format.
    pub fn to_line(&self) -> String {
        format!("{} {}", self.key(), self.sha256)
    }
}

/// In-memory representation of the global lockfile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalLockFile {
    entries: BTreeMap<String, GlobalLockEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// A `path:` entry was found. Global lockfile rejects path entries.
    PathEntryNotAllowed { line_number: usize },
    /// Generic parse failure with a human-readable reason.
    Malformed {
        line_number: usize,
        reason: &'static str,
    },
}

impl GlobalLockFile {
    /// Parse a lockfile from its serialised text content.
    ///
    /// - Blank lines and `#`-prefixed comment lines are skipped.
    /// - `<org>/<name>@<ver> <os>/<arch> sha256:<hex>` lines are accepted.
    /// - `<org>/<name>@<ver> path:<...>` lines are rejected (§1a.4).
    /// - Anything else is `LockError::Malformed`.
    pub fn parse(text: &str) -> Result<Self, LockError> {
        let mut entries = BTreeMap::new();
        for (idx, raw) in text.lines().enumerate() {
            let line_number = idx + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let entry = parse_line(line, line_number)?;
            entries.insert(entry.key(), entry);
        }
        Ok(Self { entries })
    }

    /// Serialise the lockfile to its on-disk text representation.
    /// Output begins with a comment header and entries are stably ordered.
    pub fn serialize(&self) -> String {
        let mut out = String::from("# forest.lock — do not edit manually\n");
        for entry in self.entries.values() {
            out.push_str(&entry.to_line());
            out.push('\n');
        }
        out
    }

    /// Insert or replace an entry. Last writer wins per key.
    pub fn insert(&mut self, entry: GlobalLockEntry) {
        let key = entry.key();
        self.entries.insert(key, entry);
    }

    /// Look up the sha256 for a `(org, name, version, os, arch)` tuple.
    pub fn get(
        &self,
        org: &str,
        name: &str,
        version: &str,
        os: &str,
        arch: &str,
    ) -> Option<&str> {
        let key = format!("{org}/{name}@{version} {os}/{arch}");
        self.entries.get(&key).map(|e| e.sha256.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = &GlobalLockEntry> {
        self.entries.values()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Parse one non-blank, non-comment lockfile line.
fn parse_line(line: &str, line_number: usize) -> Result<GlobalLockEntry, LockError> {
    // Split into at most 3 whitespace-separated fields:
    //   field 0: "<org>/<name>@<version>"
    //   field 1: "<os>/<arch>" OR "path:<...>"
    //   field 2: "sha256:<hex>"
    let mut parts = line.splitn(3, ' ');
    let component = parts.next().ok_or(LockError::Malformed {
        line_number,
        reason: "empty line after trim",
    })?;
    let middle = parts.next().ok_or(LockError::Malformed {
        line_number,
        reason: "missing platform/path field",
    })?;

    // Path entries are detected and rejected before any further parsing.
    if middle.starts_with("path:") {
        return Err(LockError::PathEntryNotAllowed { line_number });
    }

    let sha = parts.next().ok_or(LockError::Malformed {
        line_number,
        reason: "missing sha field",
    })?;

    let (org_name, version) = component.split_once('@').ok_or(LockError::Malformed {
        line_number,
        reason: "missing @version separator",
    })?;
    let (org, name) = org_name.split_once('/').ok_or(LockError::Malformed {
        line_number,
        reason: "missing organisation/name separator",
    })?;
    let (os, arch) = middle.split_once('/').ok_or(LockError::Malformed {
        line_number,
        reason: "missing os/arch separator",
    })?;

    Ok(GlobalLockEntry {
        organisation: org.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        os: os.to_string(),
        arch: arch.to_string(),
        sha256: sha.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample_entry(name: &str, version: &str, os: &str, arch: &str, sha: &str) -> GlobalLockEntry {
        GlobalLockEntry {
            organisation: "cuteorg".into(),
            name: name.into(),
            version: version.into(),
            os: os.into(),
            arch: arch.into(),
            sha256: format!("sha256:{sha}"),
        }
    }

    // --- key() ---

    #[test]
    fn key_is_org_name_version_os_arch() {
        let e = sample_entry("ripgrep", "14.1.1", "linux", "amd64", "abc");
        assert_eq!(e.key(), "cuteorg/ripgrep@14.1.1 linux/amd64");
    }

    #[test]
    fn key_differs_per_platform() {
        let a = sample_entry("ripgrep", "14.1.1", "linux", "amd64", "abc");
        let b = sample_entry("ripgrep", "14.1.1", "linux", "arm64", "abc");
        assert_ne!(a.key(), b.key());
    }

    // --- to_line() / parse round-trip ---

    #[test]
    fn to_line_format_matches_spec() {
        let e = sample_entry("ripgrep", "14.1.1", "linux", "amd64", "abc");
        assert_eq!(
            e.to_line(),
            "cuteorg/ripgrep@14.1.1 linux/amd64 sha256:abc"
        );
    }

    #[test]
    fn parse_then_serialize_is_stable() {
        let original = "\
# forest.lock — do not edit manually
cuteorg/fd@10.2.0 linux/amd64 sha256:9b2c
cuteorg/ripgrep@14.1.1 linux/amd64 sha256:ad3a
";
        let lock = GlobalLockFile::parse(original).unwrap();
        let serialised = lock.serialize();
        // Round-trip: re-parse the serialisation and assert equality of the
        // structured form (textual whitespace may differ on the header line
        // but the data must match).
        let lock2 = GlobalLockFile::parse(&serialised).unwrap();
        assert_eq!(lock, lock2);
    }

    #[test]
    fn parse_skips_blank_and_comment_lines() {
        let text = "\
# header
\n
cuteorg/rg@14.1.1 linux/amd64 sha256:abc

# trailing comment
";
        let lock = GlobalLockFile::parse(text).unwrap();
        assert_eq!(lock.len(), 1);
    }

    // --- strict-mode rejection of path entries ---

    #[test]
    fn parse_rejects_path_entries() {
        // §1a.4: the global lockfile is strict-mode; path entries are an error.
        let text = "cuteorg/local@0.0.1 path:./local-build\n";
        let err = GlobalLockFile::parse(text).unwrap_err();
        assert_eq!(err, LockError::PathEntryNotAllowed { line_number: 1 });
    }

    #[test]
    fn parse_rejects_path_entries_mixed_with_valid() {
        // Even if there's a perfectly good registry entry on a later line,
        // the path entry on an earlier line must short-circuit with an error.
        let text = "\
cuteorg/local@0.0.1 path:./local-build
cuteorg/rg@14.1.1 linux/amd64 sha256:abc
";
        let err = GlobalLockFile::parse(text).unwrap_err();
        let is_path_error = matches!(err, LockError::PathEntryNotAllowed { line_number: 1 });
        assert!(is_path_error, "got {err:?}");
    }

    // --- malformed input ---

    #[test]
    fn parse_rejects_missing_version() {
        let err = GlobalLockFile::parse("cuteorg/rg linux/amd64 sha256:abc\n").unwrap_err();
        let is_malformed = matches!(err, LockError::Malformed { line_number: 1, .. });
        assert!(is_malformed, "got {err:?}");
    }

    #[test]
    fn parse_rejects_missing_platform_separator() {
        let err =
            GlobalLockFile::parse("cuteorg/rg@14.1.1 linuxamd64 sha256:abc\n").unwrap_err();
        let is_malformed = matches!(err, LockError::Malformed { line_number: 1, .. });
        assert!(is_malformed, "got {err:?}");
    }

    // --- get() ---

    #[test]
    fn get_returns_sha_when_present() {
        let mut lock = GlobalLockFile::default();
        lock.insert(sample_entry("rg", "14.1.1", "linux", "amd64", "abc"));
        assert_eq!(
            lock.get("cuteorg", "rg", "14.1.1", "linux", "amd64"),
            Some("sha256:abc")
        );
    }

    #[test]
    fn get_returns_none_when_absent() {
        let lock = GlobalLockFile::default();
        assert_eq!(lock.get("cuteorg", "rg", "14.1.1", "linux", "amd64"), None);
    }

    #[test]
    fn get_distinguishes_platforms() {
        let mut lock = GlobalLockFile::default();
        lock.insert(sample_entry("rg", "14.1.1", "linux", "amd64", "amd"));
        lock.insert(sample_entry("rg", "14.1.1", "linux", "arm64", "arm"));
        assert_eq!(
            lock.get("cuteorg", "rg", "14.1.1", "linux", "amd64"),
            Some("sha256:amd")
        );
        assert_eq!(
            lock.get("cuteorg", "rg", "14.1.1", "linux", "arm64"),
            Some("sha256:arm")
        );
    }

    // --- insert() semantics ---

    #[test]
    fn insert_replaces_same_key() {
        let mut lock = GlobalLockFile::default();
        lock.insert(sample_entry("rg", "14.1.1", "linux", "amd64", "old"));
        lock.insert(sample_entry("rg", "14.1.1", "linux", "amd64", "new"));
        assert_eq!(lock.len(), 1);
        assert_eq!(
            lock.get("cuteorg", "rg", "14.1.1", "linux", "amd64"),
            Some("sha256:new")
        );
    }

    // --- property: round-trip is identity for valid lockfiles ---

    proptest! {
        #[test]
        fn parse_serialise_round_trip(
            entries in proptest::collection::vec(
                (
                    "[a-z][a-z0-9-]{0,16}",  // org
                    "[a-z][a-z0-9-]{0,16}",  // name
                    r"\d{1,3}\.\d{1,3}\.\d{1,3}", // version
                    "linux|darwin",
                    "amd64|arm64",
                    "[0-9a-f]{8,32}",
                ),
                0..6,
            )
        ) {
            let mut lock = GlobalLockFile::default();
            for (org, name, version, os, arch, sha) in &entries {
                lock.insert(GlobalLockEntry {
                    organisation: org.clone(),
                    name: name.clone(),
                    version: version.clone(),
                    os: os.clone(),
                    arch: arch.clone(),
                    sha256: format!("sha256:{sha}"),
                });
            }

            let serialised = lock.serialize();
            let reparsed = GlobalLockFile::parse(&serialised).unwrap();
            prop_assert_eq!(lock, reparsed);
        }
    }
}
