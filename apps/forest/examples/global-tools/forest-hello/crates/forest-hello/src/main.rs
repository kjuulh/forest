// forest-hello — a TOOL_BINARY example for the global-tools demo.
//
// This is a plain CLI binary. Forest invokes it via argv passthrough — there
// is no `_meta/describe`, no method dispatch, no component protocol. The
// `forest.component.cue` next to this crate declares only a #Tool facet,
// which is what makes the registry classify it as shape=TOOL_BINARY.
//
// It also demonstrates component-declared shell integration (DATA-588):
// `forest.cue` declares `include.shell.init.<shell>`, which tells forest to run
// `forest-hello shell <shell>` once when the tool is fetched and cache the
// output. `eval "$(forest shell zsh)"` then loads it — the user never adds a
// per-tool line to their rc file.

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        // The command `include.shell.init` points at. Prints an rc-file snippet
        // on stdout and nothing else — stdout here is `eval`'d by a shell.
        Some("shell") => {
            let shell = args.next().unwrap_or_else(|| "zsh".to_string());
            print!("{}", shell_integration(&shell));
        }
        Some(who) => println!("hello, {who}!"),
        None => println!("hello, anonymous!"),
    }
}

/// The integration script for `shell`. Deliberately tiny — the point of the
/// example is the declaration plumbing, not the contents.
///
/// fish is not POSIX, so it gets its own form; zsh and bash share one.
fn shell_integration(shell: &str) -> String {
    match shell {
        "fish" => "\
function hello-forest
    forest-hello $argv
end
"
        .to_string(),
        // zsh and bash: the POSIX function form is valid in both.
        _ => "\
hello-forest() {
  forest-hello \"$@\"
}
"
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_shell_gets_a_snippet() {
        // forest.cue declares zsh, bash and fish; each must produce something,
        // or the capture step caches an empty script.
        for shell in ["zsh", "bash", "fish"] {
            assert!(!shell_integration(shell).is_empty(), "{shell}");
        }
    }

    #[test]
    fn posix_shells_share_one_form_and_fish_differs() {
        // fish has no `name() { … }` function syntax, so emitting the POSIX form
        // to fish would be a syntax error at source time.
        assert_eq!(shell_integration("zsh"), shell_integration("bash"));
        assert_ne!(shell_integration("zsh"), shell_integration("fish"));
        assert!(shell_integration("fish").contains("function hello-forest"));
        assert!(shell_integration("zsh").contains("hello-forest() {"));
    }

    #[test]
    fn snippets_end_with_a_newline() {
        // The aggregate concatenates these; a missing trailing newline would
        // glue two tools' scripts onto one line.
        for shell in ["zsh", "bash", "fish"] {
            assert!(shell_integration(shell).ends_with('\n'), "{shell}");
        }
    }
}
