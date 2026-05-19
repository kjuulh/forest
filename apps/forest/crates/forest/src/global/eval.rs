//! Shell-eval script generators for `forest shell zsh` and `forest shell bash`.
//!
//! Pure module — no I/O. Output is byte-stable per TASKS/018-global-tools.md
//! §1a.7. The script prepends the shim directory to `$PATH` exactly once
//! (idempotent under repeated sourcing thanks to the `case` guard).

/// The shim directory path that the eval scripts prepend to `$PATH`.
///
/// Hard-coded as a literal in the emitted script — NOT resolved here.
/// Resolution happens in the user's shell at source-time so `$XDG_CACHE_HOME`
/// and `$HOME` expand correctly per the user's environment. The POSIX
/// `${VAR:-default}` form means "use $XDG_CACHE_HOME if it's set and non-empty,
/// otherwise fall back to $HOME/.cache" — matching the XDG Base Directory spec
/// and Forest's runtime `xdg_cache_home()` resolver in `global::paths`.
pub const SHIM_DIR_LITERAL: &str =
    "${XDG_CACHE_HOME:-$HOME/.cache}/forest/global/shims";

/// Render the zsh eval script. Byte-stable; same input always yields
/// byte-identical output.
pub fn eval_zsh() -> String {
    render()
}

/// Render the bash eval script.
///
/// The POSIX `case` form works in both shells, so the output is byte-identical
/// to `eval_zsh()`. The two functions are kept as separate entry points so the
/// CLI can dispatch on the shell name and future-proof the divergence if it
/// ever becomes necessary.
pub fn eval_bash() -> String {
    render()
}

fn render() -> String {
    format!(
        "# forest shell — adds the global shim dir to PATH idempotently\n\
         case \":$PATH:\" in\n  \
           *\":{SHIM_DIR_LITERAL}:\"*) ;;\n  \
           *) export PATH=\"{SHIM_DIR_LITERAL}:$PATH\" ;;\n\
         esac\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Determinism + idempotency guards ---

    #[test]
    fn eval_zsh_is_deterministic() {
        // §1a.7 / P5 — same input, byte-identical output.
        assert_eq!(eval_zsh(), eval_zsh());
    }

    #[test]
    fn eval_bash_is_deterministic() {
        assert_eq!(eval_bash(), eval_bash());
    }

    #[test]
    fn zsh_and_bash_emit_identical_scripts() {
        // §1a.7: the POSIX case form is valid in both shells; for this spec
        // we emit byte-identical scripts. If they diverge in a future spec,
        // this test will be updated alongside.
        assert_eq!(eval_zsh(), eval_bash());
    }

    // --- Structural invariants required for idempotency (P6) ---

    #[test]
    fn contains_idempotency_case_guard() {
        // P6 (structural lemma): emitted script contains the exact substring
        // that makes double-sourcing safe.
        let script = eval_zsh();
        let expected = format!("*\":{SHIM_DIR_LITERAL}:\"*) ;;");
        assert!(
            script.contains(&expected),
            "missing PATH-presence case guard in: {script}"
        );
    }

    #[test]
    fn case_examines_path_with_leading_and_trailing_colons() {
        // The guard wraps `$PATH` in colons so first/last entries match too.
        let script = eval_zsh();
        assert!(
            script.contains("case \":$PATH:\" in"),
            "missing canonical case header in: {script}"
        );
    }

    #[test]
    fn exports_path_with_shim_dir_prepended_on_miss() {
        let script = eval_zsh();
        let expected = format!("export PATH=\"{SHIM_DIR_LITERAL}:$PATH\"");
        assert!(
            script.contains(&expected),
            "missing PATH-prepend in: {script}"
        );
    }

    #[test]
    fn shim_dir_literal_is_used_verbatim() {
        let script = eval_zsh();
        assert!(
            script.contains(SHIM_DIR_LITERAL),
            "script must embed SHIM_DIR_LITERAL: {script}"
        );
    }

    // --- Negative: must NOT eagerly expand HOME or XDG_CACHE_HOME ---

    #[test]
    fn never_substitutes_home_at_render_time() {
        // The script must contain literal `$HOME` and `$XDG_CACHE_HOME`, never
        // their expanded values — expansion happens in the user's shell so the
        // emitted script is portable across users / environments.
        let script = eval_zsh();
        let home_expanded = std::env::var("HOME").unwrap_or_default();
        if !home_expanded.is_empty() {
            assert!(
                !script.contains(&format!("{home_expanded}/.cache/forest")),
                "script must not pre-expand $HOME at render time; got: {script}"
            );
        }
        let xdg_expanded = std::env::var("XDG_CACHE_HOME").unwrap_or_default();
        if !xdg_expanded.is_empty() {
            assert!(
                !script.contains(&format!("{xdg_expanded}/forest")),
                "script must not pre-expand $XDG_CACHE_HOME at render time; got: {script}"
            );
        }
        assert!(
            script.contains("$HOME"),
            "script must contain literal $HOME: {script}"
        );
        assert!(
            script.contains("${XDG_CACHE_HOME:-"),
            "script must honor $XDG_CACHE_HOME via POSIX default-expansion: {script}"
        );
    }

    // --- Header / shape ---

    #[test]
    fn starts_with_human_readable_comment() {
        // §1a.7 first line is a comment explaining what this script does.
        let script = eval_zsh();
        let first_line = script.lines().next().unwrap_or("");
        assert!(
            first_line.starts_with('#'),
            "first line must be a comment: {first_line:?}"
        );
    }

    #[test]
    fn ends_with_esac() {
        // The case block ends with `esac`; the script must close it.
        let trimmed = eval_zsh().trim_end().to_string();
        assert!(
            trimmed.ends_with("esac"),
            "script must end with `esac`: {trimmed}"
        );
    }
}
