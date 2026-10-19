pub const SHIM_DIR_LITERAL: &str = "${XDG_CACHE_HOME:-$HOME/.cache}/forest/global/shims";

const AGGREGATE_LITERAL: &str = "${XDG_CACHE_HOME:-$HOME/.cache}/forest/global/shell";

pub fn shell_integration_block(shell: &str) -> String {
    let (compdef_begin, compdef_end) = if shell == "zsh" {
        (
            "_forest_compdef_guard_begin\n  ",
            "\n  _forest_compdef_guard_end",
        )
    } else {
        ("", "")
    };
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
           {compdef_begin}. \"$_forest_shell_aggregate\"{compdef_end}\n\
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

pub fn eval_zsh() -> String {
    render()
}

pub fn eval_bash() -> String {
    render()
}

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

pub fn posix_path_prepend(shim_dir: &str) -> String {
    format!(
        "case \":$PATH:\" in\n  \
           *\":{shim_dir}:\"*) ;;\n  \
           *) export PATH=\"{shim_dir}:$PATH\" ;;\n\
         esac\n",
    )
}

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

    #[test]
    fn eval_zsh_is_deterministic() {
        assert_eq!(eval_zsh(), eval_zsh());
    }

    #[test]
    fn eval_bash_is_deterministic() {
        assert_eq!(eval_bash(), eval_bash());
    }

    #[test]
    fn zsh_and_bash_emit_identical_scripts() {
        assert_eq!(eval_zsh(), eval_bash());
    }

    #[test]
    fn contains_idempotency_case_guard() {
        let script = eval_zsh();
        let expected = format!("*\":{SHIM_DIR_LITERAL}:\"*) ;;");
        assert!(
            script.contains(&expected),
            "missing PATH-presence case guard in: {script}"
        );
    }

    #[test]
    fn case_examines_path_with_leading_and_trailing_colons() {
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

    #[test]
    fn never_substitutes_home_at_render_time() {
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

    #[test]
    fn starts_with_human_readable_comment() {
        let script = eval_zsh();
        let first_line = script.lines().next().unwrap_or("");
        assert!(
            first_line.starts_with('#'),
            "first line must be a comment: {first_line:?}"
        );
    }

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
        let expected = format!(
            "# forest shell — adds the global shim dir to PATH idempotently\n{}",
            posix_path_prepend(SHIM_DIR_LITERAL),
        );
        assert_eq!(eval_zsh(), expected);
    }

    #[test]
    fn ends_with_esac() {
        let trimmed = eval_zsh().trim_end().to_string();
        assert!(
            trimmed.ends_with("esac"),
            "script must end with `esac`: {trimmed}"
        );
    }

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
    fn zsh_block_guards_the_source_against_a_missing_compdef() {
        let b = shell_integration_block("zsh");
        let begin = b
            .find("_forest_compdef_guard_begin")
            .expect("begin missing");
        let src = b
            .find(". \"$_forest_shell_aggregate\"")
            .expect("source missing");
        let end = b.find("_forest_compdef_guard_end").expect("end missing");
        assert!(begin < src && src < end, "guard must wrap the source: {b}");
    }

    #[test]
    fn only_zsh_gets_the_compdef_guard() {
        assert!(!shell_integration_block("bash").contains("_forest_compdef_guard"));
        assert!(!fish_shell_integration_block().contains("_forest_compdef_guard"));
    }

    #[test]
    fn integration_block_never_execs_a_tool_at_startup() {
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
        let b = fish_shell_integration_block();
        assert!(!b.contains("${XDG_CACHE_HOME:-"), "{b}");
        assert!(b.contains("if test -n \"$XDG_CACHE_HOME\""), "{b}");
        assert!(b.contains("source "), "{b}");
        assert!(b.trim_end().ends_with("end"), "{b}");
    }

    #[test]
    fn fish_falls_back_only_when_xdg_is_empty_or_unset() {
        assert!(
            eval_fish().contains("if test -n \"$XDG_CACHE_HOME\""),
            "must gate XDG on non-empty via test -n: {}",
            eval_fish()
        );
    }
}
