//! Content-addressable binary cache (effectful).
//!
//! Reads/writes `~/.cache/forest/components/bin/<sha256>` exactly as the
//! existing components-v2 path does, but with the explicit P3 invariant
//! that `finalize` verifies sha BEFORE renaming. The warm path (`read_by_sha`)
//! trusts the content-address (§1a.9b / T1).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::global::fs::{atomic_write_executable, ensure_dir};
use crate::global::paths::GlobalPaths;

#[derive(Clone)]
pub struct BinaryCache {
    paths: GlobalPaths,
}

impl BinaryCache {
    pub fn new(paths: GlobalPaths) -> Self {
        Self { paths }
    }

    /// Locate a cached binary by sha. Returns `Some(path)` iff `bin/<sha>`
    /// exists. **Does not re-hash** — see Q9.a (content-address trust).
    pub async fn read_by_sha(&self, sha: &str) -> Result<Option<PathBuf>> {
        let p = self.paths.cached_binary(sha);
        match fs::metadata(&p).await {
            Ok(_) => Ok(Some(p)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("stat {}", p.display())),
        }
    }

    /// Verify a tempfile hashes to `expected_sha`, then atomically move into
    /// the cache at `bin/<sha>` with mode 0755. Concurrent writers producing
    /// identical bytes converge to the same content-addressed name.
    pub async fn finalize(&self, tempfile_bytes: &[u8], expected_sha: &str) -> Result<PathBuf> {
        let want_hex = expected_sha.strip_prefix("sha256:").unwrap_or(expected_sha);
        let actual = hex::encode(Sha256::digest(tempfile_bytes));
        if actual != want_hex {
            return Err(anyhow!(
                "sha mismatch — refusing to write to cache. expected={want_hex} actual={actual}"
            ));
        }
        ensure_dir(&self.paths.binary_cache_dir()).await?;
        let dest = self.paths.cached_binary(&actual);
        atomic_write_executable(&dest, tempfile_bytes).await?;
        Ok(dest)
    }

    /// Install an already-streamed tempfile (DATA-505).
    ///
    /// The sha was computed incrementally while the bytes were being written,
    /// so this verifies `computed_sha == expected_sha` and then `rename(2)`s
    /// the file into `bin/<sha>` — no second pass over the artifact and no
    /// need to hold it in memory. Same P3 invariant as [`Self::finalize`]:
    /// **verify before rename**, so a mismatch can never become a cache entry.
    ///
    /// The tempfile is removed on mismatch. `temp_path` must be on the same
    /// filesystem as the cache dir (pass the cache dir to the downloader) or
    /// the rename will fail with `EXDEV`.
    pub async fn finalize_streamed(
        &self,
        temp_path: &Path,
        computed_sha: &str,
        expected_sha: &str,
    ) -> Result<PathBuf> {
        let want_hex = expected_sha.strip_prefix("sha256:").unwrap_or(expected_sha);
        let got_hex = computed_sha.strip_prefix("sha256:").unwrap_or(computed_sha);
        if got_hex != want_hex {
            fs::remove_file(temp_path).await.ok();
            return Err(anyhow!(
                "sha mismatch — refusing to write to cache. expected={want_hex} actual={got_hex}"
            ));
        }

        ensure_dir(&self.paths.binary_cache_dir()).await?;
        let dest = self.paths.cached_binary(got_hex);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(temp_path)
                .await
                .with_context(|| format!("stat {}", temp_path.display()))?
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(temp_path, perms)
                .await
                .with_context(|| format!("chmod 0755 {}", temp_path.display()))?;
        }

        // Content-addressed destination: a concurrent writer racing us here is
        // installing byte-identical content under the same name, so whichever
        // rename lands last is still correct.
        fs::rename(temp_path, &dest)
            .await
            .with_context(|| format!("renaming {} -> {}", temp_path.display(), dest.display()))?;
        Ok(dest)
    }

    /// Walk the cache and re-hash every entry. Returns mismatched paths
    /// that were deleted. Used by `forest global verify`.
    pub async fn re_verify(&self) -> Result<Vec<PathBuf>> {
        let root = self.paths.binary_cache_dir();
        if !root.exists() {
            return Ok(vec![]);
        }
        let mut mismatched = Vec::new();
        let mut entries = fs::read_dir(&root)
            .await
            .with_context(|| format!("read_dir {}", root.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let bytes = match fs::read(&path).await {
                Ok(b) => b,
                Err(_) => continue,
            };
            let actual = hex::encode(Sha256::digest(&bytes));
            if actual != name {
                fs::remove_file(&path).await.ok();
                mismatched.push(path);
            }
        }
        Ok(mismatched)
    }
}

/// Compute the sha256 hex of an arbitrary byte slice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Stream bytes from an `AsyncRead` into a tempfile, simultaneously hashing.
/// Returns the temp path + computed sha. The caller then `finalize`s.
pub async fn write_to_tempfile(cache_root: &Path, bytes: &[u8]) -> Result<(PathBuf, String)> {
    ensure_dir(cache_root).await?;
    let rand: u64 = rand::random();
    let tmp = cache_root.join(format!(".incoming.{rand:016x}"));
    let mut file = fs::File::create(&tmp)
        .await
        .with_context(|| format!("creating tempfile {}", tmp.display()))?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    let sha = sha256_hex(bytes);
    Ok((tmp, sha))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn paths_under(td: &TempDir) -> GlobalPaths {
        GlobalPaths::with_roots(
            td.path().join("cfg"),
            td.path().join("state"),
            td.path().join("cache"),
        )
    }

    #[tokio::test]
    async fn read_by_sha_returns_none_when_absent() {
        let td = TempDir::new().unwrap();
        let c = BinaryCache::new(paths_under(&td));
        assert!(c.read_by_sha("abc").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn finalize_writes_and_read_by_sha_finds() {
        let td = TempDir::new().unwrap();
        let c = BinaryCache::new(paths_under(&td));
        let bytes = b"abc";
        let sha = sha256_hex(bytes);
        let written = c.finalize(bytes, &sha).await.unwrap();
        let found = c.read_by_sha(&sha).await.unwrap().unwrap();
        assert_eq!(written, found);
    }

    #[tokio::test]
    async fn finalize_rejects_sha_mismatch() {
        let td = TempDir::new().unwrap();
        let c = BinaryCache::new(paths_under(&td));
        let err = c
            .finalize(
                b"hello",
                "0000000000000000000000000000000000000000000000000000000000000000",
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("sha mismatch"));
    }

    #[tokio::test]
    async fn finalize_accepts_sha256_prefix() {
        let td = TempDir::new().unwrap();
        let c = BinaryCache::new(paths_under(&td));
        let bytes = b"abc";
        let prefixed = format!("sha256:{}", sha256_hex(bytes));
        c.finalize(bytes, &prefixed).await.unwrap();
        // Cached at the hex-only filename:
        assert!(c.read_by_sha(&prefixed).await.unwrap().is_some());
        assert!(c.read_by_sha(&sha256_hex(bytes)).await.unwrap().is_some());
    }

    // --- finalize_streamed (DATA-505) ---

    async fn streamed_temp(paths: &GlobalPaths, bytes: &[u8]) -> PathBuf {
        let dir = paths.binary_cache_dir();
        ensure_dir(&dir).await.unwrap();
        let tmp = dir.join(format!(".incoming.{:016x}", rand::random::<u64>()));
        tokio::fs::write(&tmp, bytes).await.unwrap();
        tmp
    }

    #[tokio::test]
    async fn finalize_streamed_installs_and_is_findable_by_sha() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"a streamed binary";
        let sha = sha256_hex(bytes);
        let tmp = streamed_temp(&paths, bytes).await;

        let dest = c.finalize_streamed(&tmp, &sha, &sha).await.unwrap();
        assert_eq!(c.read_by_sha(&sha).await.unwrap().unwrap(), dest);
        assert_eq!(tokio::fs::read(&dest).await.unwrap(), bytes);
        assert!(!tmp.exists(), "tempfile should have been renamed away");
    }

    #[tokio::test]
    async fn finalize_streamed_sets_the_executable_bit() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let td = TempDir::new().unwrap();
            let paths = paths_under(&td);
            let c = BinaryCache::new(paths.clone());
            let bytes = b"#!/bin/sh\n";
            let sha = sha256_hex(bytes);
            let tmp = streamed_temp(&paths, bytes).await;
            let dest = c.finalize_streamed(&tmp, &sha, &sha).await.unwrap();
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[tokio::test]
    async fn finalize_streamed_rejects_mismatch_and_writes_nothing() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"tampered payload";
        let actual = sha256_hex(bytes);
        let expected = "0".repeat(64);
        let tmp = streamed_temp(&paths, bytes).await;

        let err = c
            .finalize_streamed(&tmp, &actual, &expected)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("sha mismatch"), "{err}");
        assert!(
            c.read_by_sha(&expected).await.unwrap().is_none(),
            "a mismatched download must never become a cache entry"
        );
        assert!(
            c.read_by_sha(&actual).await.unwrap().is_none(),
            "and must not be cached under its own sha either"
        );
        assert!(!tmp.exists(), "the rejected tempfile should be cleaned up");
    }

    #[tokio::test]
    async fn finalize_streamed_accepts_the_sha256_prefix_on_either_side() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"prefixed";
        let sha = sha256_hex(bytes);

        let tmp = streamed_temp(&paths, bytes).await;
        c.finalize_streamed(&tmp, &sha, &format!("sha256:{sha}"))
            .await
            .unwrap();
        assert!(c.read_by_sha(&sha).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn concurrent_streamed_finalize_of_identical_content_converges() {
        // Two downloads of the same artifact racing to install it. Both must
        // succeed and the cache must end up with the one content-addressed
        // entry.
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"raced content";
        let sha = sha256_hex(bytes);

        let a = streamed_temp(&paths, bytes).await;
        let b = streamed_temp(&paths, bytes).await;
        let (ra, rb) = tokio::join!(
            c.finalize_streamed(&a, &sha, &sha),
            c.finalize_streamed(&b, &sha, &sha)
        );
        assert_eq!(ra.unwrap(), rb.unwrap());
        assert_eq!(
            tokio::fs::read(paths.cached_binary(&sha)).await.unwrap(),
            bytes
        );

        let mut entries = tokio::fs::read_dir(paths.binary_cache_dir()).await.unwrap();
        let mut names = Vec::new();
        while let Some(e) = entries.next_entry().await.unwrap() {
            names.push(e.file_name().to_string_lossy().to_string());
        }
        assert_eq!(names, vec![sha], "no stray tempfiles, exactly one entry");
    }

    #[tokio::test]
    async fn re_verify_deletes_mismatched_entries() {
        let td = TempDir::new().unwrap();
        let c = BinaryCache::new(paths_under(&td));
        let bytes = b"hello";
        let sha = sha256_hex(bytes);
        c.finalize(bytes, &sha).await.unwrap();

        // Corrupt the cached file.
        let path = c.paths.cached_binary(&sha);
        tokio::fs::write(&path, b"tampered").await.unwrap();

        let deleted = c.re_verify().await.unwrap();
        assert_eq!(deleted.len(), 1);
        assert!(c.read_by_sha(&sha).await.unwrap().is_none());
    }
}
