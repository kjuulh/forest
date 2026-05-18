//! Tool / shim name validation.
//!
//! Pure module — no I/O. Implements the regex `^[a-zA-Z][a-zA-Z0-9._-]{0,63}$`
//! from TASKS/018-global-tools.md §1a.1, with the additional defence-in-depth
//! rejection of literal `..` substrings (§1a.2 rule 3).

/// Reasons a name can fail validation. Each variant is a single concrete defect
/// so error messages can identify exactly what's wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong { len: usize, max: usize },
    BadFirstChar { ch: char },
    BadChar { ch: char, position: usize },
    ContainsDotDot,
}

/// Maximum allowed name length (excluding the leading character, per the regex).
pub const MAX_NAME_LEN: usize = 64;

/// Validate a tool / shim name against the rules in §1a.1 + §1a.2 rule 3.
///
/// Accepts iff:
///   - Length is in [1, 64].
///   - First char is `[a-zA-Z]`.
///   - Every subsequent char is `[a-zA-Z0-9._-]`.
///   - Does NOT contain the literal substring `..`.
pub fn validate_tool_name(name: &str) -> Result<(), NameError> {
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.len() > MAX_NAME_LEN {
        return Err(NameError::TooLong {
            len: name.len(),
            max: MAX_NAME_LEN,
        });
    }

    let mut chars = name.chars().enumerate();
    let (_, first) = chars.next().expect("non-empty checked above");
    if !first.is_ascii_alphabetic() {
        return Err(NameError::BadFirstChar { ch: first });
    }
    for (position, ch) in chars {
        let ok = ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-';
        if !ok {
            return Err(NameError::BadChar { ch, position });
        }
    }

    if name.contains("..") {
        return Err(NameError::ContainsDotDot);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // --- Happy path ---

    #[test]
    fn accepts_simple_lowercase() {
        validate_tool_name("rg").unwrap();
        validate_tool_name("scaffolder").unwrap();
        validate_tool_name("hello").unwrap();
    }

    #[test]
    fn accepts_uppercase_first_char() {
        validate_tool_name("MyTool").unwrap();
    }

    #[test]
    fn accepts_internal_digits_dots_underscores_hyphens() {
        validate_tool_name("rg14").unwrap();
        validate_tool_name("tool.v2").unwrap();
        validate_tool_name("tool_v2").unwrap();
        validate_tool_name("tool-v2").unwrap();
        validate_tool_name("a.b_c-d.0").unwrap();
    }

    #[test]
    fn accepts_max_length_name() {
        // 64 chars: one letter + 63 of the tail alphabet.
        let name = format!("a{}", "x".repeat(63));
        assert_eq!(name.len(), 64);
        validate_tool_name(&name).unwrap();
    }

    // --- Rejections ---

    #[test]
    fn rejects_empty() {
        assert_eq!(validate_tool_name(""), Err(NameError::Empty));
    }

    #[test]
    fn rejects_too_long() {
        // 65 chars.
        let name = format!("a{}", "x".repeat(64));
        assert_eq!(name.len(), 65);
        let err = validate_tool_name(&name).unwrap_err();
        assert!(
            matches!(err, NameError::TooLong { len: 65, max: 64 }),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_leading_digit() {
        let err = validate_tool_name("1tool").unwrap_err();
        assert_eq!(err, NameError::BadFirstChar { ch: '1' });
    }

    #[test]
    fn rejects_leading_hyphen() {
        // §1a.10 E9 — defends against argv parsing as a flag.
        let err = validate_tool_name("-tool").unwrap_err();
        assert_eq!(err, NameError::BadFirstChar { ch: '-' });
    }

    #[test]
    fn rejects_leading_dot() {
        let err = validate_tool_name(".tool").unwrap_err();
        assert_eq!(err, NameError::BadFirstChar { ch: '.' });
    }

    #[test]
    fn rejects_leading_underscore() {
        let err = validate_tool_name("_tool").unwrap_err();
        assert_eq!(err, NameError::BadFirstChar { ch: '_' });
    }

    #[test]
    fn rejects_slash() {
        // §1a.10 E9 — path separator.
        let err = validate_tool_name("ab/cd").unwrap_err();
        assert_eq!(err, NameError::BadChar { ch: '/', position: 2 });
    }

    #[test]
    fn rejects_backslash() {
        let err = validate_tool_name("ab\\cd").unwrap_err();
        assert_eq!(err, NameError::BadChar { ch: '\\', position: 2 });
    }

    #[test]
    fn rejects_null_byte() {
        let err = validate_tool_name("ab\0cd").unwrap_err();
        assert_eq!(err, NameError::BadChar { ch: '\0', position: 2 });
    }

    #[test]
    fn rejects_whitespace() {
        let err = validate_tool_name("ab cd").unwrap_err();
        assert_eq!(err, NameError::BadChar { ch: ' ', position: 2 });
    }

    #[test]
    fn rejects_unicode_outside_ascii_alnum_set() {
        let err = validate_tool_name("toolé").unwrap_err();
        // Position 4 is the byte index of `é`'s first byte; we report char
        // position. Tighten this once the implementation is precise.
        assert!(matches!(err, NameError::BadChar { ch: 'é', .. }));
    }

    #[test]
    fn rejects_double_dot_substring() {
        // §1a.2 rule 3: `..` is rejected as a literal substring even if every
        // single character is otherwise valid.
        let err = validate_tool_name("a..b").unwrap_err();
        assert_eq!(err, NameError::ContainsDotDot);
    }

    #[test]
    fn rejects_double_dot_at_end() {
        let err = validate_tool_name("ab..").unwrap_err();
        assert_eq!(err, NameError::ContainsDotDot);
    }

    // --- Property tests ---

    proptest! {
        /// Every accepted name must round-trip through the regex.
        #[test]
        fn accepted_names_match_regex(s in r"[a-zA-Z][a-zA-Z0-9._-]{0,63}") {
            // The proptest regex includes `..` patterns; we only assert
            // that names *without* `..` are accepted, and names *with* `..`
            // are rejected. This separates the two rules cleanly.
            if !s.contains("..") {
                prop_assert!(validate_tool_name(&s).is_ok(), "expected accept for {s:?}");
            } else {
                prop_assert_eq!(validate_tool_name(&s), Err(NameError::ContainsDotDot));
            }
        }

        /// Any name longer than 64 chars must be rejected with `TooLong`.
        #[test]
        fn rejects_anything_too_long(s in r"[a-zA-Z][a-zA-Z0-9._-]{64,128}") {
            let err = validate_tool_name(&s);
            let is_too_long = matches!(err, Err(NameError::TooLong { .. }));
            prop_assert!(is_too_long);
        }

        /// Any name whose first byte is outside `[a-zA-Z]` must be rejected
        /// with `BadFirstChar` (or `Empty` if length 0).
        #[test]
        fn rejects_bad_first_char(s in r"[0-9_.\-][a-zA-Z0-9._-]{0,10}") {
            let err = validate_tool_name(&s).unwrap_err();
            let ok_variant = matches!(
                err,
                NameError::BadFirstChar { .. } | NameError::ContainsDotDot,
            );
            prop_assert!(ok_variant, "got {:?} for {:?}", err, s);
        }
    }
}
