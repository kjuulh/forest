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

// --- Environment variable names / values ------------------------------------
//
// Default env vars shipped in a component's `include` block (TASKS/023). The
// name rule is the conventional POSIX env-name regex `^[A-Za-z_][A-Za-z0-9_]*$`;
// values may contain anything except a NUL byte (the one byte `Command::env`
// cannot carry on Unix). These are the single source of truth — the manifest
// parser and the user-config parser both call them.

/// Reasons an env-var name can fail validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvNameError {
    Empty,
    BadFirstChar { ch: char },
    BadChar { ch: char, position: usize },
}

/// Reasons an env-var value can fail validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValueError {
    ContainsNul { position: usize },
}

/// Validate an env-var name against `^[A-Za-z_][A-Za-z0-9_]*$`.
pub fn validate_env_name(name: &str) -> Result<(), EnvNameError> {
    let mut chars = name.chars().enumerate();
    let (_, first) = chars.next().ok_or(EnvNameError::Empty)?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(EnvNameError::BadFirstChar { ch: first });
    }
    for (position, ch) in chars {
        if !(ch.is_ascii_alphanumeric() || ch == '_') {
            return Err(EnvNameError::BadChar { ch, position });
        }
    }
    Ok(())
}

/// Validate an env-var value: anything but a NUL byte is allowed.
pub fn validate_env_value(value: &str) -> Result<(), EnvValueError> {
    match value.find('\0') {
        Some(position) => Err(EnvValueError::ContainsNul { position }),
        None => Ok(()),
    }
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
        assert_eq!(
            err,
            NameError::BadChar {
                ch: '/',
                position: 2
            }
        );
    }

    #[test]
    fn rejects_backslash() {
        let err = validate_tool_name("ab\\cd").unwrap_err();
        assert_eq!(
            err,
            NameError::BadChar {
                ch: '\\',
                position: 2
            }
        );
    }

    #[test]
    fn rejects_null_byte() {
        let err = validate_tool_name("ab\0cd").unwrap_err();
        assert_eq!(
            err,
            NameError::BadChar {
                ch: '\0',
                position: 2
            }
        );
    }

    #[test]
    fn rejects_whitespace() {
        let err = validate_tool_name("ab cd").unwrap_err();
        assert_eq!(
            err,
            NameError::BadChar {
                ch: ' ',
                position: 2
            }
        );
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

    // --- Env name / value validation ---

    #[test]
    fn env_name_accepts_conventional_names() {
        validate_env_name("FUNGUS_SERVER").unwrap();
        validate_env_name("_private").unwrap();
        validate_env_name("RUST_LOG").unwrap();
        validate_env_name("X").unwrap();
        validate_env_name("a1_b2").unwrap();
    }

    #[test]
    fn env_name_rejects_empty() {
        assert_eq!(validate_env_name(""), Err(EnvNameError::Empty));
    }

    #[test]
    fn env_name_rejects_leading_digit() {
        assert_eq!(
            validate_env_name("1FOO"),
            Err(EnvNameError::BadFirstChar { ch: '1' })
        );
    }

    #[test]
    fn env_name_rejects_hyphen_and_dot() {
        assert_eq!(
            validate_env_name("FOO-BAR"),
            Err(EnvNameError::BadChar {
                ch: '-',
                position: 3
            })
        );
        assert_eq!(
            validate_env_name("FOO.BAR"),
            Err(EnvNameError::BadChar {
                ch: '.',
                position: 3
            })
        );
    }

    #[test]
    fn env_value_allows_anything_but_nul() {
        validate_env_value("").unwrap();
        validate_env_value("https://fungus.understory.sh").unwrap();
        validate_env_value("multi\nline = value with spaces / é").unwrap();
    }

    #[test]
    fn env_value_rejects_nul() {
        assert_eq!(
            validate_env_value("ab\0cd"),
            Err(EnvValueError::ContainsNul { position: 2 })
        );
    }

    proptest! {
        /// Names matching the POSIX regex are always accepted.
        #[test]
        fn env_name_accepts_regex(s in r"[A-Za-z_][A-Za-z0-9_]{0,32}") {
            prop_assert!(validate_env_name(&s).is_ok(), "expected accept for {s:?}");
        }

        /// Any string free of NUL is a valid value.
        #[test]
        fn env_value_accepts_non_nul(s in r"[^\x00]{0,64}") {
            prop_assert!(validate_env_value(&s).is_ok());
        }
    }
}
