//! Pure path canonicalisation + archive entry selection.
//!
//! Implements TASKS/018-global-tools.md §1a.2d and the property P12 target.
//! No I/O.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    Empty,
    TooLong,
    AbsolutePath,
    HomeExpansion,
    BackslashByte,
    NullByte,
    NewlineByte,
    DotSegment,
    DotDotSegment,
    HiddenSegment,
    NonNfc,
}

/// Canonicalise a `binary_in_archive` path per §1a.2d.
///
/// Accepts only forward-slash separated relative paths whose segments are
/// non-empty, non-dot, non-double-dot, non-hidden, and contain no
/// `0x00 / 0x0A / 0x0D / 0x5C` bytes. Returns the path verbatim on success
/// (we don't rewrite — the canonical form IS what the caller passed).
pub fn canonicalise(path: &str) -> Result<String, PathError> {
    if path.is_empty() {
        return Err(PathError::Empty);
    }
    if path.len() > 256 {
        return Err(PathError::TooLong);
    }
    if path.starts_with('/') {
        return Err(PathError::AbsolutePath);
    }
    if path.starts_with('~') {
        return Err(PathError::HomeExpansion);
    }
    // Byte-level early checks (cheaper than per-segment).
    for b in path.bytes() {
        match b {
            0 => return Err(PathError::NullByte),
            b'\n' | b'\r' => return Err(PathError::NewlineByte),
            b'\\' => return Err(PathError::BackslashByte),
            _ => {}
        }
    }
    // Segment checks.
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err(PathError::DotSegment); // collapses //, leading/trailing /
        }
        if segment == "." {
            return Err(PathError::DotSegment);
        }
        if segment == ".." {
            return Err(PathError::DotDotSegment);
        }
        if segment.starts_with('.') {
            return Err(PathError::HiddenSegment);
        }
    }
    // NFC normalisation: the canonical form for comparison is NFC. Inputs
    // that are not already in NFC are rejected so that downstream byte-equal
    // comparisons against archive entries are stable. (Selectors and archive
    // entries are normalised symmetrically — see `select`.)
    if !is_nfc(path) {
        return Err(PathError::NonNfc);
    }
    Ok(path.to_string())
}

fn is_nfc(s: &str) -> bool {
    use unicode_normalization::UnicodeNormalization;
    s.nfc().eq(s.chars())
}

/// Normalise a string to NFC. Used by the archive selector to compare
/// archive-entry names that may be in NFD against canonicalised targets.
fn to_nfc(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfc().collect()
}

/// Result of selecting one entry from a list of archive entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectError {
    /// Target is not a valid canonicalisable path.
    InvalidTarget(PathError),
    /// No entry matched.
    NotFound { target: String, available: Vec<String> },
    /// More than one entry matched the canonical target.
    Ambiguous { target: String, matches: Vec<usize> },
}

/// Pick exactly one entry from `entries` whose canonicalised name equals
/// `target`. Property P12 (path-traversal safety): only returns `Ok` when
/// `target` itself canonicalises AND uniquely matches a canonicalisable entry.
pub fn select(entries: &[String], target: &str) -> Result<usize, SelectError> {
    let canon_target = canonicalise(target).map_err(SelectError::InvalidTarget)?;

    let mut matching = Vec::new();
    let mut available = Vec::with_capacity(entries.len());
    for (idx, entry) in entries.iter().enumerate() {
        // Archives commonly emit NFD on some filesystems (notably macOS HFS+);
        // normalise to NFC before canonicalisation so the comparison against
        // an NFC `target` is byte-equal-safe.
        let normalised = to_nfc(entry);
        match canonicalise(&normalised) {
            Ok(canon) => {
                if canon == canon_target {
                    matching.push(idx);
                }
                available.push(canon);
            }
            Err(_) => {
                // Skip non-canonicalisable entries — they could be e.g. a
                // top-level directory entry in the tarball. They are never
                // selectable.
            }
        }
    }

    match matching.len() {
        0 => Err(SelectError::NotFound {
            target: canon_target,
            available,
        }),
        1 => Ok(matching[0]),
        _ => Err(SelectError::Ambiguous {
            target: canon_target,
            matches: matching,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // --- canonicalise: happy paths ----------------------------------------

    #[test]
    fn canonicalises_simple_relative_path() {
        assert_eq!(canonicalise("rg").unwrap(), "rg");
        assert_eq!(canonicalise("bin/rg").unwrap(), "bin/rg");
        assert_eq!(canonicalise("a/b/c/d/e").unwrap(), "a/b/c/d/e");
    }

    #[test]
    fn canonicalises_ripgrep_archive_path() {
        // The exact form from examples/global-tools/forest-ripgrep/forest.cue.
        let p = "ripgrep-14.1.1-x86_64-unknown-linux-musl/rg";
        assert_eq!(canonicalise(p).unwrap(), p);
    }

    // --- canonicalise: rejections -----------------------------------------

    #[test]
    fn rejects_empty() {
        assert_eq!(canonicalise(""), Err(PathError::Empty));
    }

    #[test]
    fn rejects_too_long() {
        let long = "a".repeat(257);
        assert_eq!(canonicalise(&long), Err(PathError::TooLong));
    }

    #[test]
    fn rejects_absolute_path() {
        assert_eq!(canonicalise("/etc/passwd"), Err(PathError::AbsolutePath));
    }

    #[test]
    fn rejects_home_expansion() {
        assert_eq!(canonicalise("~/.bashrc"), Err(PathError::HomeExpansion));
    }

    #[test]
    fn rejects_dotdot_segment() {
        assert_eq!(canonicalise("../etc/passwd"), Err(PathError::DotDotSegment));
        assert_eq!(canonicalise("a/../b"), Err(PathError::DotDotSegment));
        assert_eq!(canonicalise("a/.."), Err(PathError::DotDotSegment));
    }

    #[test]
    fn rejects_dot_segment() {
        assert_eq!(canonicalise("./rg"), Err(PathError::DotSegment));
        assert_eq!(canonicalise("a/./b"), Err(PathError::DotSegment));
    }

    #[test]
    fn rejects_hidden_segment() {
        assert_eq!(canonicalise(".git/config"), Err(PathError::HiddenSegment));
        assert_eq!(canonicalise("a/.hidden/b"), Err(PathError::HiddenSegment));
    }

    #[test]
    fn rejects_consecutive_slashes() {
        assert_eq!(canonicalise("a//b"), Err(PathError::DotSegment));
    }

    #[test]
    fn rejects_trailing_slash() {
        assert_eq!(canonicalise("a/b/"), Err(PathError::DotSegment));
    }

    #[test]
    fn rejects_backslash() {
        assert_eq!(canonicalise("a\\b"), Err(PathError::BackslashByte));
    }

    #[test]
    fn rejects_null_byte() {
        assert_eq!(canonicalise("a\0b"), Err(PathError::NullByte));
    }

    #[test]
    fn rejects_newline_byte() {
        assert_eq!(canonicalise("a\nb"), Err(PathError::NewlineByte));
        assert_eq!(canonicalise("a\rb"), Err(PathError::NewlineByte));
    }

    #[test]
    fn accepts_nfc_non_ascii_paths() {
        // `café` written in NFC (single codepoint U+00E9 for `é`) — accepted.
        let nfc = "café/x";
        let out = canonicalise(nfc).unwrap();
        assert_eq!(out, nfc);
    }

    #[test]
    fn rejects_nfd_non_ascii_paths() {
        // `café` in NFD: `e` (U+0065) followed by combining acute (U+0301).
        let nfd = "cafe\u{0301}/x";
        assert_eq!(canonicalise(nfd), Err(PathError::NonNfc));
    }

    #[test]
    fn select_normalises_archive_entries_to_nfc() {
        // The archive yields an entry in NFD, but the manifest target is NFC.
        // The selector must normalise the entry before comparison.
        let nfd_entry = "cafe\u{0301}/binary".to_string();
        let nfc_target = "café/binary";
        let entries = vec![nfd_entry];
        let idx = select(&entries, nfc_target).unwrap();
        assert_eq!(idx, 0);
    }

    // --- select() ---------------------------------------------------------

    #[test]
    fn select_finds_single_match() {
        let entries = vec![
            "ripgrep-14.1.1-x86_64-unknown-linux-musl/COPYING".to_string(),
            "ripgrep-14.1.1-x86_64-unknown-linux-musl/rg".to_string(),
            "ripgrep-14.1.1-x86_64-unknown-linux-musl/README.md".to_string(),
        ];
        let idx = select(&entries, "ripgrep-14.1.1-x86_64-unknown-linux-musl/rg").unwrap();
        assert_eq!(idx, 1);
    }

    #[test]
    fn select_rejects_invalid_target() {
        let entries = vec!["rg".to_string()];
        let err = select(&entries, "../etc/passwd").unwrap_err();
        let is_invalid =
            matches!(err, SelectError::InvalidTarget(PathError::DotDotSegment));
        assert!(is_invalid, "got {err:?}");
    }

    #[test]
    fn select_returns_not_found_when_missing() {
        let entries = vec!["bin/other".to_string()];
        let err = select(&entries, "bin/rg").unwrap_err();
        let is_not_found = matches!(err, SelectError::NotFound { .. });
        assert!(is_not_found, "got {err:?}");
    }

    #[test]
    fn select_returns_ambiguous_on_duplicate() {
        // Real tarballs don't usually contain duplicate paths, but the
        // archive selector must not pick one arbitrarily.
        let entries = vec!["bin/rg".to_string(), "bin/rg".to_string()];
        let err = select(&entries, "bin/rg").unwrap_err();
        let is_ambig = matches!(err, SelectError::Ambiguous { ref matches, .. } if matches.len() == 2);
        assert!(is_ambig, "got {err:?}");
    }

    #[test]
    fn select_ignores_non_canonicalisable_entries() {
        // A directory entry "bin/" in a tarball is non-canonicalisable
        // (trailing slash → empty segment). It must not block selection of
        // a legitimate entry below it.
        let entries = vec!["bin/".to_string(), "bin/rg".to_string()];
        let idx = select(&entries, "bin/rg").unwrap();
        assert_eq!(idx, 1);
    }

    // --- P12 property: select is total + traversal-safe -------------------

    proptest! {
        /// P12: for any list of entry-strings + any target, the selector
        /// returns a result without panicking AND any `Ok(idx)` implies the
        /// target is well-formed.
        #[test]
        fn p12_select_is_total_and_safe(
            entries in proptest::collection::vec("[a-zA-Z0-9./_-]{0,32}", 0..8),
            target in "[a-zA-Z0-9./_-]{0,32}",
        ) {
            let r = select(&entries, &target);
            if let Ok(idx) = r {
                // Target must be canonicalisable.
                prop_assert!(canonicalise(&target).is_ok());
                // Entry must equal canonicalised target.
                let canon = canonicalise(&target).unwrap();
                prop_assert_eq!(&entries[idx], &canon);
            }
        }

        /// Any target containing `..` is always rejected.
        #[test]
        fn p12_target_with_dotdot_always_rejected(
            prefix in "[a-z]{0,8}/",
            suffix in "/[a-z]{0,8}",
            entries in proptest::collection::vec("[a-z./]{0,16}", 0..4),
        ) {
            let target = format!("{prefix}..{suffix}");
            let r = select(&entries, &target);
            let is_invalid = matches!(r, Err(SelectError::InvalidTarget(_)));
            prop_assert!(is_invalid);
        }
    }
}
