fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("shell") => {
            let shell = args.next().unwrap_or_else(|| "zsh".to_string());
            print!("{}", shell_integration(&shell));
        }
        Some(who) => println!("hello, {who}!"),
        None => println!("hello, anonymous!"),
    }
}

fn shell_integration(shell: &str) -> String {
    match shell {
        "fish" => "\
function hello-forest
    forest-hello $argv
end
"
        .to_string(),
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
        for shell in ["zsh", "bash", "fish"] {
            assert!(!shell_integration(shell).is_empty(), "{shell}");
        }
    }

    #[test]
    fn posix_shells_share_one_form_and_fish_differs() {
        assert_eq!(shell_integration("zsh"), shell_integration("bash"));
        assert_ne!(shell_integration("zsh"), shell_integration("fish"));
        assert!(shell_integration("fish").contains("function hello-forest"));
        assert!(shell_integration("zsh").contains("hello-forest() {"));
    }

    #[test]
    fn snippets_end_with_a_newline() {
        for shell in ["zsh", "bash", "fish"] {
            assert!(shell_integration(shell).ends_with('\n'), "{shell}");
        }
    }
}
