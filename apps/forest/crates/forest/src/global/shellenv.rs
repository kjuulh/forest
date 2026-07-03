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

use crate::global::eval::{posix_path_prepend, SHIM_DIR_LITERAL};

/// First line of the managed block. Also the needle the installer/uninstaller
/// search for.
pub const BLOCK_BEGIN: &str = "# >>> forest shell (managed) >>>";
/// Last line of the managed block.
pub const BLOCK_END: &str = "# <<< forest shell (managed) <<<";

/// The shells we manage an env file for. Explicit (not `cfg!`) so both are
/// unit-testable on any host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Zsh,
    Bash,
}

impl Shell {
    pub fn name(self) -> &'static str {
        match self {
            Shell::Zsh => "zsh",
            Shell::Bash => "bash",
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
    pub fn rc_file(self, home: &Path) -> PathBuf {
        match self {
            Shell::Zsh => home.join(".zshenv"),
            Shell::Bash => home.join(".bashrc"),
        }
    }
}

/// The managed block to write into an rc file: the marker-delimited,
/// `case`-guarded PATH prepend (the same shared guard as `forest shell zsh`,
/// against the unexpanded [`SHIM_DIR_LITERAL`] so `$HOME`/`$XDG_CACHE_HOME`
/// expand per-user at source time).
///
/// Identical for zsh and bash — the POSIX `case` form is valid in both.
/// Deterministic: same input, byte-identical output.
pub fn managed_block() -> String {
    format!(
        "{BLOCK_BEGIN}\n\
         # Added by `forest shell install` so spawned (non-interactive) shells\n\
         # find forest-installed tools. Remove with `forest shell uninstall`.\n\
         {}{BLOCK_END}\n",
        posix_path_prepend(SHIM_DIR_LITERAL),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_block_is_deterministic() {
        assert_eq!(managed_block(), managed_block());
    }

    #[test]
    fn block_is_delimited_by_both_markers() {
        let b = managed_block();
        assert!(b.starts_with(BLOCK_BEGIN), "must start with begin marker: {b}");
        assert!(b.trim_end().ends_with(BLOCK_END), "must end with end marker: {b}");
    }

    #[test]
    fn block_embeds_the_shared_guarded_prepend_verbatim() {
        // Single source of truth: the block reuses eval::posix_path_prepend
        // against the literal, exactly like `forest shell zsh`.
        assert!(managed_block().contains(&posix_path_prepend(SHIM_DIR_LITERAL)));
    }

    #[test]
    fn block_does_not_pre_expand_home_or_xdg() {
        // Expansion happens in the user's shell at source time.
        let b = managed_block();
        assert!(b.contains("$HOME"), "{b}");
        assert!(b.contains("${XDG_CACHE_HOME:-"), "{b}");
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
}
