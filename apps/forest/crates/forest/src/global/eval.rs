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
pub const SHIM_DIR_LITERAL: &str = "${XDG_CACHE_HOME:-$HOME/.cache}/forest/global/shims";

/// The per-shell aggregate script that `forest shell <shell>` sources — the
/// concatenation of every installed tool's component-declared shell integration
/// (DATA-588). Same unexpanded-literal discipline as [`SHIM_DIR_LITERAL`]:
/// `$HOME`/`$XDG_CACHE_HOME` expand in the user's shell, not here.
///
/// `{shell}` is substituted by [`shell_integration_block`]; the resolved
/// counterpart is [`crate::global::paths::GlobalPaths::shell_aggregate`].
const AGGREGATE_LITERAL: &str = "${XDG_CACHE_HOME:-$HOME/.cache}/forest/global/shell";

/// Render the POSIX block that loads component-declared shell integrations
/// (DATA-588).
///
/// This is what replaces a hand-maintained pile of `eval "$(<tool> init zsh)"`
/// lines in the user's rc file. Components declare `include.shell.init.<shell>`
/// in their manifest; forest captures each tool's script when the tool is
/// fetched and concatenates them into one aggregate file. Startup therefore
/// costs a single `source` of a single file — no process per tool, and above all
/// no lazy download standing between the user and their prompt.
///
/// `FOREST_NO_SHELL_INTEGRATION=1` opts out entirely — nothing sourced, no warm
/// started. This exists because the block runs in every interactive shell and
/// sources third-party script: when that goes wrong, the way back to a working
/// shell must not be "edit your rc file".
///
/// Cold cache (nothing captured yet, so no aggregate): kick off a detached,
/// silent, throttled warm and arm the deferred loader, so integrations arrive in
/// *this* shell a moment later instead of blocking it. The `forest` binary is
/// always present — it is what emitted this block — so that call can't itself
/// trigger a download.
///
/// `shell` selects the aggregate; deterministic per shell. POSIX form — fish
/// gets [`fish_shell_integration_block`].
pub fn shell_integration_block(shell: &str) -> String {
    format!(
        "\n\
         # forest shell — component-declared tool integrations (DATA-588).\n\
         # Tools declare `include.shell.init.{shell}` in their manifest; forest\n\
         # captures each script once and concatenates them here, so this costs one\n\
         # file read rather than one process (or one download) per tool.\n\
         _forest_shell_aggregate=\"{AGGREGATE_LITERAL}/{shell}.sh\"\n\
         if [ -n \"${{FOREST_NO_SHELL_INTEGRATION:-}}\" ]; then\n  \
           : # opted out — load nothing, start nothing\n\
         elif [ -r \"$_forest_shell_aggregate\" ]; then\n  \
           . \"$_forest_shell_aggregate\"\n\
         else\n  \
           # Nothing captured yet (fresh install, or a cold cache). Warm in the\n  \
           # background — detached, silent, throttled — and load the aggregate as\n  \
           # soon as it lands, rather than downloading tools at startup.\n  \
           forest global warm --background --quiet 2>/dev/null\n  \
           forest-defer-aggregate\n\
         fi\n\
         unset _forest_shell_aggregate\n",
    )
}

/// Fish counterpart of [`shell_integration_block`]. Same intent, fish syntax:
/// no POSIX `${VAR:-default}` (hence the `test -n` guard, matching
/// [`fish_path_prepend`]) and `source` instead of `.`.
pub fn fish_shell_integration_block() -> String {
    "\n\
     # forest shell — component-declared tool integrations (DATA-588).\n\
     # Tools declare `include.shell.init.fish` in their manifest; forest captures\n\
     # each script once and concatenates them here, so this costs one file read\n\
     # rather than one process (or one download) per tool.\n\
     set -l forest_shell_aggregate $HOME/.cache/forest/global/shell/fish.sh\n\
     if test -n \"$XDG_CACHE_HOME\"\n    \
         set forest_shell_aggregate $XDG_CACHE_HOME/forest/global/shell/fish.sh\n\
     end\n\
     if test -n \"$FOREST_NO_SHELL_INTEGRATION\"\n    \
         # opted out — load nothing, start nothing\n\
     else if test -r \"$forest_shell_aggregate\"\n    \
         source \"$forest_shell_aggregate\"\n\
     else\n    \
         # Nothing captured yet (fresh install, or a cold cache). Warm in the\n    \
         # background — detached, silent, throttled — and load the aggregate as\n    \
         # soon as it lands, rather than downloading tools at startup.\n    \
         forest global warm --background --quiet 2>/dev/null\n    \
         forest-defer-aggregate\n\
     end\n"
        .to_string()
}

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

/// Render the fish eval script.
///
/// Fish is not POSIX: it has no `case`/`${VAR:-default}`, so it gets its own
/// [`fish_path_prepend`] guard instead of [`posix_path_prepend`]. Deterministic:
/// same input, byte-identical output.
pub fn eval_fish() -> String {
    format!(
        "# forest shell — adds the global shim dir to PATH idempotently\n{}",
        fish_path_prepend(),
    )
}

fn render() -> String {
    format!(
        "# forest shell — adds the global shim dir to PATH idempotently\n{}",
        posix_path_prepend(SHIM_DIR_LITERAL),
    )
}

/// Render the POSIX `case`-guarded PATH-prepend for `shim_dir`.
///
/// This is the single source of truth for Forest's idempotent PATH injection:
/// the `case` guard makes it safe to run more than once (double-sourcing a
/// shell rc, re-running a launchd agent) because a second run finds `shim_dir`
/// already present and does nothing. `eval_zsh`/`eval_bash` embed it with the
/// unexpanded [`SHIM_DIR_LITERAL`]; the macOS LaunchAgent generator in
/// `global::pathenv` embeds it with the *resolved* absolute shim dir (launchd
/// has no `$XDG_CACHE_HOME`/`$HOME` to expand).
///
/// `shim_dir` is interpolated verbatim — callers pass either the literal or a
/// pre-resolved absolute path. Deterministic: same input, byte-identical
/// output.
pub fn posix_path_prepend(shim_dir: &str) -> String {
    format!(
        "case \":$PATH:\" in\n  \
           *\":{shim_dir}:\"*) ;;\n  \
           *) export PATH=\"{shim_dir}:$PATH\" ;;\n\
         esac\n",
    )
}

/// Render the fish idempotent PATH-prepend for the shim dir.
///
/// The fish analogue of [`posix_path_prepend`]: same intent (prepend the shim
/// dir to `$PATH` exactly once, safe to re-source), expressed in fish syntax.
/// Fish lacks POSIX `${VAR:-default}` so the `$XDG_CACHE_HOME`-with-`$HOME`
/// fallback is a `test -n`/`set` guard (matching POSIX "unset OR empty →
/// default"), and idempotency comes from fish's `contains` rather than a `case`
/// block. Resolution happens in the user's shell at source time, so `$HOME` and
/// `$XDG_CACHE_HOME` are emitted literally, never pre-expanded here.
///
/// Deterministic: no input, byte-identical output every call.
pub fn fish_path_prepend() -> String {
    "set -l forest_shim_dir $HOME/.cache/forest/global/shims\n\
     if test -n \"$XDG_CACHE_HOME\"\n    \
         set forest_shim_dir $XDG_CACHE_HOME/forest/global/shims\n\
     end\n\
     if not contains -- $forest_shim_dir $PATH\n    \
         set -gx PATH $forest_shim_dir $PATH\n\
     end\n"
        .to_string()
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

    // --- Shared prepend helper (reused by global::pathenv) ---

    #[test]
    fn posix_path_prepend_is_guarded_and_prepends() {
        let s = posix_path_prepend("/abs/shims");
        assert!(s.contains("case \":$PATH:\" in"), "{s}");
        assert!(s.contains("*\":/abs/shims:\"*) ;;"), "{s}");
        assert!(s.contains("export PATH=\"/abs/shims:$PATH\""), "{s}");
        assert!(s.trim_end().ends_with("esac"), "{s}");
    }

    #[test]
    fn render_reuses_posix_path_prepend_with_the_literal() {
        // The eval script is exactly the header comment + the shared prepend
        // rendered against SHIM_DIR_LITERAL. Guards against the two drifting.
        let expected = format!(
            "# forest shell — adds the global shim dir to PATH idempotently\n{}",
            posix_path_prepend(SHIM_DIR_LITERAL),
        );
        assert_eq!(eval_zsh(), expected);
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

    // --- fish (non-POSIX; separate generator) ---

    #[test]
    fn eval_fish_is_deterministic() {
        assert_eq!(eval_fish(), eval_fish());
    }

    #[test]
    fn fish_starts_with_human_readable_comment() {
        let script = eval_fish();
        let first_line = script.lines().next().unwrap_or("");
        assert!(
            first_line.starts_with('#'),
            "first line must be a comment: {first_line:?}"
        );
    }

    #[test]
    fn fish_uses_contains_guard_not_posix_case() {
        // Idempotency in fish comes from `contains`, and fish has no POSIX
        // `case`/`esac` — emitting those would be a syntax error under fish.
        let script = eval_fish();
        assert!(
            script.contains("if not contains -- $forest_shim_dir $PATH"),
            "missing fish contains guard: {script}"
        );
        assert!(
            !script.contains("case "),
            "must not emit POSIX case: {script}"
        );
        assert!(!script.contains("esac"), "must not emit esac: {script}");
    }

    #[test]
    fn fish_prepends_shim_dir_on_miss() {
        assert!(
            eval_fish().contains("set -gx PATH $forest_shim_dir $PATH"),
            "missing fish PATH-prepend: {}",
            eval_fish()
        );
    }

    #[test]
    fn fish_never_pre_expands_home_or_xdg() {
        // Same portability rule as POSIX: expansion happens in the user's shell.
        let script = eval_fish();
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            assert!(
                !script.contains(&format!("{home}/.cache/forest")),
                "must not pre-expand $HOME: {script}"
            );
        }
        assert!(
            script.contains("$HOME/.cache"),
            "must contain literal $HOME: {script}"
        );
        assert!(
            script.contains("$XDG_CACHE_HOME"),
            "must contain literal $XDG_CACHE_HOME: {script}"
        );
    }

    // --- component-declared shell integration (DATA-588) ------------------

    #[test]
    fn integration_block_is_deterministic_per_shell() {
        for shell in ["zsh", "bash"] {
            assert_eq!(
                shell_integration_block(shell),
                shell_integration_block(shell)
            );
        }
        assert_ne!(
            shell_integration_block("zsh"),
            shell_integration_block("bash"),
            "each shell must source its own aggregate"
        );
        assert_eq!(
            fish_shell_integration_block(),
            fish_shell_integration_block()
        );
    }

    #[test]
    fn integration_block_sources_the_aggregate_for_its_shell() {
        for shell in ["zsh", "bash"] {
            let b = shell_integration_block(shell);
            assert!(
                b.contains(&format!("{AGGREGATE_LITERAL}/{shell}.sh")),
                "{b}"
            );
        }
        assert!(
            fish_shell_integration_block().contains("forest/global/shell/fish.sh"),
            "{}",
            fish_shell_integration_block()
        );
    }

    #[test]
    fn integration_block_guards_on_readability_before_sourcing() {
        // A fresh install has no aggregate; sourcing unconditionally would print
        // "no such file" above the user's first prompt on every new shell.
        for shell in ["zsh", "bash"] {
            assert!(
                shell_integration_block(shell).contains("if [ -r \"$_forest_shell_aggregate\" ]"),
                "{}",
                shell_integration_block(shell)
            );
        }
        assert!(
            fish_shell_integration_block().contains("if test -r \"$forest_shell_aggregate\""),
            "{}",
            fish_shell_integration_block()
        );
    }

    #[test]
    fn cold_cache_branch_warms_in_background_and_defers() {
        // The two halves of the non-blocking promise: never download inline, and
        // still get the integrations into *this* shell.
        for b in [
            shell_integration_block("zsh"),
            shell_integration_block("bash"),
            fish_shell_integration_block(),
        ] {
            assert!(
                b.contains("forest global warm --background --quiet"),
                "must warm out-of-band: {b}"
            );
            assert!(
                b.contains("forest-defer-aggregate"),
                "must arm the deferred loader: {b}"
            );
        }
    }

    #[test]
    fn integration_block_never_execs_a_tool_at_startup() {
        // The whole point: startup reads a file. Any per-tool invocation here
        // would reintroduce the cold-cache download it exists to remove.
        for b in [
            shell_integration_block("zsh"),
            shell_integration_block("bash"),
            fish_shell_integration_block(),
        ] {
            assert!(!b.contains("global run"), "{b}");
            assert!(!b.contains("forest-init"), "{b}");
        }
    }

    #[test]
    fn integration_block_does_not_pre_expand_home_or_xdg() {
        // Same portability rule as the PATH prepend: expansion happens in the
        // user's shell, so one emitted block is valid for any user.
        for b in [
            shell_integration_block("zsh"),
            shell_integration_block("bash"),
            fish_shell_integration_block(),
        ] {
            assert!(b.contains("$HOME"), "{b}");
            assert!(b.contains("XDG_CACHE_HOME"), "{b}");
        }
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            assert!(!shell_integration_block("zsh").contains(&format!("{home}/.cache/forest")));
        }
    }

    #[test]
    fn posix_block_cleans_up_its_temporary_variable() {
        // The block runs in the user's interactive shell; leaving
        // `_forest_shell_aggregate` set would leak a forest-internal name into
        // their environment.
        for shell in ["zsh", "bash"] {
            assert!(
                shell_integration_block(shell).contains("unset _forest_shell_aggregate"),
                "{}",
                shell_integration_block(shell)
            );
        }
    }

    #[test]
    fn fish_block_uses_fish_syntax_only() {
        // fish has no POSIX `[ -r … ]`, `.` sourcing, or `${VAR:-default}`.
        let b = fish_shell_integration_block();
        assert!(!b.contains("${XDG_CACHE_HOME:-"), "{b}");
        assert!(b.contains("if test -n \"$XDG_CACHE_HOME\""), "{b}");
        assert!(b.contains("source "), "{b}");
        assert!(b.trim_end().ends_with("end"), "{b}");
    }

    #[test]
    fn fish_falls_back_only_when_xdg_is_empty_or_unset() {
        // `test -n` matches POSIX ${VAR:-default} (default on unset OR empty),
        // unlike fish's `set -q` which treats empty as set.
        assert!(
            eval_fish().contains("if test -n \"$XDG_CACHE_HOME\""),
            "must gate XDG on non-empty via test -n: {}",
            eval_fish()
        );
    }
}
