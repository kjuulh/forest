//! Pure generators for the "make forest tools discoverable to spawned,
//! non-interactive shells" install step (DATA-420).
//!
//! The real failure mode isn't GUI apps — it's that `forest shell zsh|bash` is
//! sourced from `~/.zshrc`, which zsh reads **only for interactive** shells.
//! When a tool (e.g. Claude Code) spawns `zsh -c` / `bash -c`, that shell is
//! non-interactive and skips `.zshrc`, so the shim dir is on PATH only if it
//! happened to be inherited from an interactive ancestor. Break that chain and
//! forest tools vanish.
//!
//! The fix is placement, not machinery: put the PATH prepend in the file each
//! shell reads on **every** invocation — `~/.zshenv` for zsh (`.zshrc` is
//! interactive-only), `~/.bashrc` for bash. This module produces the managed
//! block to insert; the effectful reader/writer lives in
//! [`crate::global::install`].
//!
//! The block is delimited by [`BLOCK_BEGIN`]/[`BLOCK_END`] markers so the
//! installer can insert-or-replace it and the uninstaller can excise exactly
//! it, leaving the rest of the user's rc file untouched — the same
//! recognise-then-act discipline the shim sync uses via `SHIM_MARKER`.

use std::path::{Path, PathBuf};

use crate::global::eval::{fish_path_prepend, posix_path_prepend, SHIM_DIR_LITERAL};

/// First line of the managed block. Also the needle the installer/uninstaller
/// search for.
pub const BLOCK_BEGIN: &str = "# >>> forest shell (managed) >>>";
/// Last line of the managed block.
pub const BLOCK_END: &str = "# <<< forest shell (managed) <<<";

/// The shells we manage an env file for. Explicit (not `cfg!`) so all are
/// unit-testable on any host.
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

    /// The rc file that this shell reads on **every** invocation (crucially
    /// including non-interactive `-c`), relative to `home`.
    ///
    /// - zsh → `~/.zshenv` (always sourced; `.zshrc` is interactive-only).
    /// - bash → `~/.bashrc` (interactive; the closest universal bash file —
    ///   note that a bare `bash -c` reads nothing unless `$BASH_ENV` is set, so
    ///   bash coverage leans on PATH inheritance from a `.zshenv`-fixed
    ///   ancestor; documented in `forest docs shell`).
    /// - fish → `~/.config/fish/conf.d/forest.fish` (fish auto-sources every
    ///   file in `conf.d` for ALL fish shells, interactive or not — the fish
    ///   analogue of zsh's `.zshenv`, and the reason fish gets full coverage a
    ///   bare `bash -c` can't).
    pub fn rc_file(self, home: &Path) -> PathBuf {
        match self {
            Shell::Zsh => home.join(".zshenv"),
            Shell::Bash => home.join(".bashrc"),
            Shell::Fish => home.join(".config/fish/conf.d/forest.fish"),
        }
    }
}

/// The managed block to write into `shell`'s rc file: the marker-delimited
/// PATH prepend, using the unexpanded shim-dir literal so `$HOME`/
/// `$XDG_CACHE_HOME` expand per-user at source time.
///
/// The prepend body is the SAME guard `forest shell <shell>` emits: the POSIX
/// `case` form for zsh/bash (valid in both), and the fish `contains` form for
/// fish (which is not POSIX). The `#`-comment markers are valid in all three.
/// Deterministic: same input, byte-identical output.
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
            assert!(b.starts_with(BLOCK_BEGIN), "must start with begin marker: {b}");
            assert!(b.trim_end().ends_with(BLOCK_END), "must end with end marker: {b}");
        }
    }

    #[test]
    fn posix_block_embeds_the_shared_guarded_prepend_verbatim() {
        // Single source of truth: the zsh/bash block reuses posix_path_prepend
        // against the literal, exactly like `forest shell zsh`.
        for shell in [Shell::Zsh, Shell::Bash] {
            assert!(managed_block(shell).contains(&posix_path_prepend(SHIM_DIR_LITERAL)));
        }
    }

    #[test]
    fn fish_block_embeds_the_fish_prepend_and_no_posix_case() {
        // Fish is not POSIX — its block must use the fish guard, never `case`.
        let b = managed_block(Shell::Fish);
        assert!(b.contains(&fish_path_prepend()), "{b}");
        assert!(!b.contains("case \":$PATH:\""), "fish block must not embed POSIX case: {b}");
    }

    #[test]
    fn block_does_not_pre_expand_home_or_xdg() {
        // Expansion happens in the user's shell at source time.
        for shell in [Shell::Zsh, Shell::Bash, Shell::Fish] {
            let b = managed_block(shell);
            assert!(b.contains("$HOME"), "{b}");
            assert!(b.contains("XDG_CACHE_HOME"), "{b}");
        }
    }

    #[test]
    fn zsh_targets_zshenv_not_zshrc() {
        // .zshenv is the whole point — .zshrc is interactive-only.
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
        // conf.d is fish's every-invocation dir — the analogue of .zshenv.
        let home = PathBuf::from("/home/u");
        assert_eq!(
            Shell::Fish.rc_file(&home),
            PathBuf::from("/home/u/.config/fish/conf.d/forest.fish")
        );
    }
}
