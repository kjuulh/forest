use std::path::{Path, PathBuf};

use crate::global::eval::{SHIM_DIR_LITERAL, fish_path_prepend, posix_path_prepend};

pub const BLOCK_BEGIN: &str = "# >>> forest shell (managed) >>>";
pub const BLOCK_END: &str = "# <<< forest shell (managed) <<<";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
}

impl Shell {
    pub fn name(self) -> &'static str {
        match self {
            Shell::Zsh => "zsh",
            Shell::Bash => "bash",
            Shell::Fish => "fish",
        }
    }

    pub fn rc_file(self, home: &Path) -> PathBuf {
        match self {
            Shell::Zsh => home.join(".zshenv"),
            Shell::Bash => home.join(".bashrc"),
            Shell::Fish => home.join(".config/fish/conf.d/forest.fish"),
        }
    }
}

pub fn managed_block(shell: Shell) -> String {
    let prepend = match shell {
        Shell::Zsh | Shell::Bash => posix_path_prepend(SHIM_DIR_LITERAL),
        Shell::Fish => fish_path_prepend(),
    };
    format!(
        "{BLOCK_BEGIN}\n\
         # Added by `forest shell install` so spawned (non-interactive) shells\n\
         # find forest-installed tools. Remove with `forest shell uninstall`.\n\
         {prepend}{BLOCK_END}\n",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_block_is_deterministic() {
        for shell in [Shell::Zsh, Shell::Bash, Shell::Fish] {
            assert_eq!(managed_block(shell), managed_block(shell));
        }
    }

    #[test]
    fn block_is_delimited_by_both_markers() {
        for shell in [Shell::Zsh, Shell::Bash, Shell::Fish] {
            let b = managed_block(shell);
            assert!(
                b.starts_with(BLOCK_BEGIN),
                "must start with begin marker: {b}"
            );
            assert!(
                b.trim_end().ends_with(BLOCK_END),
                "must end with end marker: {b}"
            );
        }
    }

    #[test]
    fn posix_block_embeds_the_shared_guarded_prepend_verbatim() {
        for shell in [Shell::Zsh, Shell::Bash] {
            assert!(managed_block(shell).contains(&posix_path_prepend(SHIM_DIR_LITERAL)));
        }
    }

    #[test]
    fn fish_block_embeds_the_fish_prepend_and_no_posix_case() {
        let b = managed_block(Shell::Fish);
        assert!(b.contains(&fish_path_prepend()), "{b}");
        assert!(
            !b.contains("case \":$PATH:\""),
            "fish block must not embed POSIX case: {b}"
        );
    }

    #[test]
    fn block_does_not_pre_expand_home_or_xdg() {
        for shell in [Shell::Zsh, Shell::Bash, Shell::Fish] {
            let b = managed_block(shell);
            assert!(b.contains("$HOME"), "{b}");
            assert!(b.contains("XDG_CACHE_HOME"), "{b}");
        }
    }

    #[test]
    fn zsh_targets_zshenv_not_zshrc() {
        let home = PathBuf::from("/home/u");
        assert_eq!(Shell::Zsh.rc_file(&home), PathBuf::from("/home/u/.zshenv"));
    }

    #[test]
    fn bash_targets_bashrc() {
        let home = PathBuf::from("/home/u");
        assert_eq!(Shell::Bash.rc_file(&home), PathBuf::from("/home/u/.bashrc"));
    }

    #[test]
    fn fish_targets_confd_which_is_always_sourced() {
        let home = PathBuf::from("/home/u");
        assert_eq!(
            Shell::Fish.rc_file(&home),
            PathBuf::from("/home/u/.config/fish/conf.d/forest.fish")
        );
    }
}
