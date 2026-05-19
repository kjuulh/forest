//! Filesystem primitives for the global-tools layer.
//!
//! - Atomic writes (tempfile + fsync + rename) — covers `forest.cue`,
//!   `forest.lock`, shim files, cached binaries.
//! - Read helpers for config + lockfile.
//! - Lazy directory creation.
//!
//! No locking primitive yet — flock comes in a separate small module once
//! per-process write-paths actually start to race.

use std::path::Path;

use anyhow::{Context, Result};
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Atomically write `bytes` to `dest` with mode 0644 (data files).
///
/// Steps:
///   1. Ensure parent dir exists.
///   2. Create a sibling tempfile in the same directory (so rename(2) is atomic).
///   3. Write + fsync the bytes.
///   4. Rename onto `dest`.
pub async fn atomic_write(dest: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_with_mode(dest, bytes, 0o644).await
}

/// Atomically write `bytes` to `dest` with mode 0755 — for executables/shims.
pub async fn atomic_write_executable(dest: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_with_mode(dest, bytes, 0o755).await
}

async fn atomic_write_with_mode(dest: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating parent dir for {}", dest.display()))?;
    }
    let tmp = sibling_tempfile(dest)?;
    {
        let mut file = fs::File::create(&tmp)
            .await
            .with_context(|| format!("creating tempfile {}", tmp.display()))?;
        file.write_all(bytes)
            .await
            .with_context(|| format!("writing tempfile {}", tmp.display()))?;
        file.sync_all()
            .await
            .with_context(|| format!("fsyncing tempfile {}", tmp.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tmp)?.permissions();
        perms.set_mode(mode);
        std::fs::set_permissions(&tmp, perms)?;
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }
    fs::rename(&tmp, dest)
        .await
        .with_context(|| format!("renaming {} -> {}", tmp.display(), dest.display()))?;
    Ok(())
}

fn sibling_tempfile(dest: &Path) -> Result<std::path::PathBuf> {
    let parent = dest.parent().ok_or_else(|| {
        anyhow::anyhow!("destination has no parent dir: {}", dest.display())
    })?;
    let stem = dest
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("destination has no file name: {}", dest.display()))?
        .to_string_lossy()
        .to_string();
    let rand: u64 = rand::random();
    Ok(parent.join(format!(".{stem}.tmp.{rand:016x}")))
}

/// Read a file to a String if it exists; return None if absent.
pub async fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path).await {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => {
            Err(e).with_context(|| format!("reading {}", path.display()))
        }
    }
}

/// Ensure a directory exists (creates with default umask).
pub async fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .await
        .with_context(|| format!("creating dir {}", path.display()))?;
    Ok(())
}

/// Delete a file if it exists; silent if not.
pub async fn remove_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn atomic_write_creates_file_with_contents() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("nested/under/here.txt");
        atomic_write(&dest, b"hello").await.unwrap();
        let got = tokio::fs::read_to_string(&dest).await.unwrap();
        assert_eq!(got, "hello");
    }

    #[tokio::test]
    async fn atomic_write_replaces_existing_file() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("x.txt");
        atomic_write(&dest, b"first").await.unwrap();
        atomic_write(&dest, b"second").await.unwrap();
        let got = tokio::fs::read_to_string(&dest).await.unwrap();
        assert_eq!(got, "second");
    }

    #[tokio::test]
    async fn atomic_write_executable_sets_0755() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = TempDir::new().unwrap();
            let dest = dir.path().join("shim");
            atomic_write_executable(&dest, b"#!/bin/sh\necho hi\n")
                .await
                .unwrap();
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[tokio::test]
    async fn read_optional_returns_none_when_absent() {
        let dir = TempDir::new().unwrap();
        let got = read_optional(&dir.path().join("missing")).await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn read_optional_returns_some_when_present() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("x");
        atomic_write(&p, b"contents").await.unwrap();
        let got = read_optional(&p).await.unwrap();
        assert_eq!(got.as_deref(), Some("contents"));
    }

    #[tokio::test]
    async fn remove_if_present_is_noop_for_missing() {
        let dir = TempDir::new().unwrap();
        remove_if_present(&dir.path().join("missing"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn remove_if_present_deletes_existing() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("x");
        atomic_write(&p, b"x").await.unwrap();
        remove_if_present(&p).await.unwrap();
        assert!(!p.exists());
    }
}
