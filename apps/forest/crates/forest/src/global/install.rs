//! Effectful reader/writer for the shell-rc PATH install step (DATA-420).
//!
//! The *content* comes from the pure [`crate::global::shellenv`] generator; this
//! module is the only part allowed to touch the filesystem. It inserts,
//! replaces, or excises a marker-delimited managed block in the user's shell
//! env files (`~/.zshenv`, `~/.bashrc`) so spawned non-interactive shells find
//! the shim dir. Writes are atomic ([`crate::global::fs::atomic_write`]); block
//! edits are string-level and never disturb the rest of the file.
//!
//! Idempotent: re-running when the block already matches is a no-op; a stale
//! block is replaced in place. Reversible: uninstall removes exactly the block
//! (matched by its markers) and nothing else.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::global::fs::{atomic_write, read_optional};
use crate::global::shellenv::{managed_block, Shell, BLOCK_BEGIN, BLOCK_END};

/// Outcome of installing the block into one rc file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The block was newly inserted (file created or block appended).
    Added(PathBuf),
    /// A stale block was replaced in place.
    Updated(PathBuf),
    /// The block was already present and current — no write.
    Unchanged(PathBuf),
}

impl Applied {
    pub fn path(&self) -> &Path {
        match self {
            Applied::Added(p) | Applied::Updated(p) | Applied::Unchanged(p) => p,
        }
    }
}

/// Outcome of uninstalling from one rc file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removed {
    /// The block was found and excised.
    Removed(PathBuf),
    /// No managed block present (or file absent) — nothing to do.
    Absent(PathBuf),
}

/// Resolve `$HOME` and the shells to target for the live environment. Targets
/// the subset of {zsh, bash} whose binary is on PATH; falls back to zsh if
/// neither resolves (zsh is the primary fix and always present on macOS).
pub fn resolve_targets() -> Result<(PathBuf, Vec<Shell>)> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("home dir is unset"))?;
    let mut shells: Vec<Shell> = [Shell::Zsh, Shell::Bash]
        .into_iter()
        .filter(|s| on_path(s.name()))
        .collect();
    if shells.is_empty() {
        shells.push(Shell::Zsh);
    }
    Ok((home, shells))
}

/// Every shell we know how to manage — used by uninstall so it cleans up files
/// even for a shell whose binary has since been removed.
pub fn all_shells() -> Vec<Shell> {
    vec![Shell::Zsh, Shell::Bash]
}

fn on_path(bin: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file())
}

/// Insert-or-replace the managed block in `rc_file`. Idempotent.
pub async fn apply(rc_file: &Path) -> Result<Applied> {
    let block = managed_block();
    let existing = read_optional(rc_file).await?;
    let (new_contents, action) = upsert_block(existing.as_deref().unwrap_or(""), &block, rc_file);
    if let Applied::Unchanged(_) = action {
        return Ok(action);
    }
    atomic_write(rc_file, new_contents.as_bytes())
        .await
        .with_context(|| format!("writing {}", rc_file.display()))?;
    Ok(action)
}

/// Remove the managed block from `rc_file`, leaving everything else intact.
pub async fn uninstall(rc_file: &Path) -> Result<Removed> {
    let Some(existing) = read_optional(rc_file).await? else {
        return Ok(Removed::Absent(rc_file.to_path_buf()));
    };
    match strip_block(&existing) {
        None => Ok(Removed::Absent(rc_file.to_path_buf())),
        Some(stripped) => {
            atomic_write(rc_file, stripped.as_bytes())
                .await
                .with_context(|| format!("writing {}", rc_file.display()))?;
            Ok(Removed::Removed(rc_file.to_path_buf()))
        }
    }
}

/// Render the dry-run plan: for each target file, what the resulting managed
/// block would be. No I/O.
pub fn render_dry_run(home: &Path, shells: &[Shell]) -> String {
    let block = managed_block();
    let mut s = String::new();
    for shell in shells {
        let rc = shell.rc_file(home);
        s.push_str(&format!("--- {} ({}) ---\n", rc.display(), shell.name()));
        s.push_str(&block);
    }
    s
}

// --- pure block-editing core ------------------------------------------------

/// Locate the managed block's byte range `[begin, end)` in `content`, where
/// `end` is just past the newline following the end marker (or EOF).
fn block_range(content: &str) -> Option<(usize, usize)> {
    let begin = content.find(BLOCK_BEGIN)?;
    let end_marker = content[begin..].find(BLOCK_END)? + begin;
    let after = end_marker + BLOCK_END.len();
    let end = content[after..]
        .find('\n')
        .map(|i| after + i + 1)
        .unwrap_or(content.len());
    Some((begin, end))
}

/// Insert or replace the block. Returns the new file contents and what changed.
fn upsert_block(content: &str, block: &str, path: &Path) -> (String, Applied) {
    if let Some((begin, end)) = block_range(content) {
        if &content[begin..end] == block {
            return (content.to_string(), Applied::Unchanged(path.to_path_buf()));
        }
        let mut out = String::with_capacity(content.len());
        out.push_str(&content[..begin]);
        out.push_str(block);
        out.push_str(&content[end..]);
        return (out, Applied::Updated(path.to_path_buf()));
    }
    // Append with a single blank-line separator from any existing content.
    if content.is_empty() {
        (block.to_string(), Applied::Added(path.to_path_buf()))
    } else {
        let base = content.trim_end_matches('\n');
        (
            format!("{base}\n\n{block}"),
            Applied::Added(path.to_path_buf()),
        )
    }
}

/// Remove the block (and the blank-line separator we inserted before it, if
/// present). Returns `None` if there is no block to remove.
fn strip_block(content: &str) -> Option<String> {
    let (begin, end) = block_range(content)?;
    // Absorb a single preceding blank line ("\n\n") so removal doesn't leave a
    // widening gap across install/uninstall cycles.
    let cut_begin = if content[..begin].ends_with("\n\n") {
        begin - 1
    } else {
        begin
    };
    let mut out = String::with_capacity(content.len());
    out.push_str(&content[..cut_begin]);
    out.push_str(&content[end..]);
    // Collapse to at most one trailing newline.
    let trimmed = out.trim_end_matches('\n');
    if trimmed.is_empty() {
        Some(String::new())
    } else {
        Some(format!("{trimmed}\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn block() -> String {
        managed_block()
    }

    // --- pure upsert/strip ---

    #[test]
    fn upsert_into_empty_yields_just_the_block() {
        let (out, act) = upsert_block("", &block(), Path::new("/x"));
        assert_eq!(out, block());
        assert!(matches!(act, Applied::Added(_)));
    }

    #[test]
    fn upsert_appends_with_blank_separator_and_preserves_existing() {
        let existing = "export EDITOR=vim\n";
        let (out, act) = upsert_block(existing, &block(), Path::new("/x"));
        assert!(matches!(act, Applied::Added(_)));
        assert!(out.starts_with("export EDITOR=vim\n\n"), "{out}");
        assert!(out.contains(BLOCK_BEGIN) && out.contains(BLOCK_END));
        // User content untouched.
        assert!(out.contains("export EDITOR=vim"));
    }

    #[test]
    fn upsert_is_idempotent() {
        let (once, _) = upsert_block("alias l=ls\n", &block(), Path::new("/x"));
        let (twice, act) = upsert_block(&once, &block(), Path::new("/x"));
        assert_eq!(once, twice);
        assert!(matches!(act, Applied::Unchanged(_)));
    }

    #[test]
    fn upsert_replaces_a_stale_block_in_place() {
        let stale = format!(
            "pre\n\n{BLOCK_BEGIN}\nexport PATH=\"/old/shims:$PATH\"\n{BLOCK_END}\npost\n"
        );
        let (out, act) = upsert_block(&stale, &block(), Path::new("/x"));
        assert!(matches!(act, Applied::Updated(_)));
        assert!(out.starts_with("pre\n"), "{out}");
        assert!(out.trim_end().ends_with("post"), "{out}");
        assert!(!out.contains("/old/shims"), "stale content must be gone: {out}");
        assert!(out.contains("forest/global/shims"), "{out}");
        // Exactly one managed block.
        assert_eq!(out.matches(BLOCK_BEGIN).count(), 1);
    }

    #[test]
    fn strip_removes_block_and_separator_preserving_neighbours() {
        let (installed, _) = upsert_block("before\n", &block(), Path::new("/x"));
        let stripped = strip_block(&installed).expect("had a block");
        assert_eq!(stripped, "before\n", "should restore the original file");
    }

    #[test]
    fn strip_returns_none_when_no_block() {
        assert!(strip_block("just user config\n").is_none());
    }

    #[test]
    fn install_then_uninstall_round_trips_to_original() {
        // Property: for arbitrary surrounding content, install+uninstall is
        // identity (modulo trailing-newline normalisation).
        for original in ["", "x\n", "a\nb\nc\n", "no-trailing-newline"] {
            let (installed, _) = upsert_block(original, &block(), Path::new("/x"));
            let restored = strip_block(&installed).unwrap_or_default();
            let norm = |s: &str| {
                let t = s.trim_end_matches('\n');
                if t.is_empty() { String::new() } else { format!("{t}\n") }
            };
            assert_eq!(norm(&restored), norm(original), "original was {original:?}");
        }
    }

    // --- effectful apply/uninstall ---

    #[tokio::test]
    async fn apply_creates_then_unchanged_then_uninstall() {
        let dir = TempDir::new().unwrap();
        let rc = dir.path().join(".zshenv");

        let a = apply(&rc).await.unwrap();
        assert!(matches!(a, Applied::Added(_)));
        let body = tokio::fs::read_to_string(&rc).await.unwrap();
        assert!(body.contains(BLOCK_BEGIN) && body.contains("forest/global/shims"));

        let a2 = apply(&rc).await.unwrap();
        assert!(matches!(a2, Applied::Unchanged(_)), "got {a2:?}");

        let r = uninstall(&rc).await.unwrap();
        assert!(matches!(r, Removed::Removed(_)));
        assert!(!tokio::fs::read_to_string(&rc).await.unwrap().contains(BLOCK_BEGIN));

        let r2 = uninstall(&rc).await.unwrap();
        assert!(matches!(r2, Removed::Absent(_)), "got {r2:?}");
    }

    #[tokio::test]
    async fn apply_preserves_existing_rc_content() {
        let dir = TempDir::new().unwrap();
        let rc = dir.path().join(".zshenv");
        atomic_write(&rc, b"export FOO=bar\nalias g=git\n").await.unwrap();

        apply(&rc).await.unwrap();
        let body = tokio::fs::read_to_string(&rc).await.unwrap();
        assert!(body.contains("export FOO=bar"));
        assert!(body.contains("alias g=git"));
        assert!(body.contains(BLOCK_BEGIN));

        uninstall(&rc).await.unwrap();
        let after = tokio::fs::read_to_string(&rc).await.unwrap();
        assert_eq!(after, "export FOO=bar\nalias g=git\n", "user content must survive");
    }

    #[test]
    fn dry_run_lists_each_target_and_the_block_without_writing() {
        let home = PathBuf::from("/home/u");
        let out = render_dry_run(&home, &[Shell::Zsh, Shell::Bash]);
        assert!(out.contains("/home/u/.zshenv"));
        assert!(out.contains("/home/u/.bashrc"));
        assert!(out.contains(BLOCK_BEGIN));
    }
}
