//! Content-addressable binary cache (effectful).
//!
//! Reads/writes `~/.cache/forest/components/bin/<sha256>/<name>` with the
//! explicit P3 invariant that `finalize` verifies sha BEFORE renaming. The warm
//! path (`read_by_sha`) trusts the content-address (§1a.9b / T1).
//!
//! **Layout (DATA-510).** The sha256 addresses a *directory*; the executable
//! sits inside it under the component's real name. `forest global run` execs
//! that path, so the child's `argv[0]` basename is the tool name rather than a
//! hex digest — which busybox-style dispatch, usage text, and self-re-exec all
//! read. A single hash may be materialised under more than one name (an alias,
//! or two components whose artifacts hash identically); the extra names are
//! hard links to the first, so they cost an inode and nothing else.
//!
//! **Migration.** Caches written before DATA-510 have `bin/<sha>` as a plain
//! *file*. Every entry point here migrates that shape in place, lazily, on
//! first touch — see [`BinaryCache::migrate_legacy_entry`]. Already-installed
//! tools keep working and are never re-downloaded.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::global::fs::{atomic_write_executable, ensure_dir};
use crate::global::paths::GlobalPaths;
use forest_manifest::names::validate_tool_name;

#[derive(Clone)]
pub struct BinaryCache {
    paths: GlobalPaths,
}

impl BinaryCache {
    pub fn new(paths: GlobalPaths) -> Self {
        Self { paths }
    }

    /// Locate a cached binary by sha, under the name it should be exec'd as.
    /// Returns `Some(path)` iff `bin/<sha>/<bin_name>` is present (or can be
    /// materialised from a sibling name of the same content).
    /// **Does not re-hash** — see Q9.a (content-address trust).
    ///
    /// Migrates a pre-DATA-510 `bin/<sha>` file into the new shape first, so
    /// the first run after an upgrade fixes the store instead of refetching.
    pub async fn read_by_sha(&self, sha: &str, bin_name: &str) -> Result<Option<PathBuf>> {
        let bin_name = checked_bin_name(bin_name)?;

        // Migration briefly moves the legacy file out of the way, so a second
        // process can look in the window where the entry is neither the old
        // file nor the new directory. Reporting a miss there would be safe but
        // wasteful — it re-downloads a tool that is already on disk, and on
        // the offline warm path it would fail outright. So a miss that
        // coincides with an in-flight migration waits for the mover to land.
        let mut waited = std::time::Duration::ZERO;
        let step = std::time::Duration::from_millis(10);
        let limit = std::time::Duration::from_millis(500);
        loop {
            self.migrate_legacy_entry(sha, bin_name).await?;
            if let Some(p) = self.lookup(sha, bin_name).await? {
                return Ok(Some(p));
            }
            // Not a race — or a mover that died holding the file, in which
            // case the cold path re-fetches. Either way, stop waiting.
            if waited >= limit || !self.migration_in_flight(sha).await {
                return Ok(None);
            }
            tokio::time::sleep(step).await;
            waited += step;
        }
    }

    /// Locate `bin/<sha>/<bin_name>`, materialising it from a sibling name of
    /// the same content when only the name is missing.
    async fn lookup(&self, sha: &str, bin_name: &str) -> Result<Option<PathBuf>> {
        let p = self.paths.cached_binary(sha, bin_name);
        match fs::metadata(&p).await {
            Ok(_) => return Ok(Some(p)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("stat {}", p.display())),
        }

        // The content is here under a different name — same bytes, different
        // component or alias. Link it into place rather than re-downloading.
        if link_from_sibling(&self.paths.cached_binary_dir(sha), &p).await? {
            return Ok(Some(p));
        }
        Ok(None)
    }

    /// Whether another process is part-way through migrating this sha — its
    /// staged file is still sitting in the cache root under the scratch name.
    async fn migration_in_flight(&self, sha: &str) -> bool {
        let hex = sha.strip_prefix("sha256:").unwrap_or(sha);
        let prefix = format!(".migrating.{hex}.");
        let Ok(mut entries) = fs::read_dir(self.paths.binary_cache_dir()).await else {
            return false;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(&prefix))
            {
                return true;
            }
        }
        false
    }

    /// Verify a tempfile hashes to `expected_sha`, then atomically move into
    /// the cache at `bin/<sha>/<bin_name>` with mode 0755. Concurrent writers
    /// producing identical bytes converge to the same content-addressed name.
    pub async fn finalize(
        &self,
        tempfile_bytes: &[u8],
        expected_sha: &str,
        bin_name: &str,
    ) -> Result<PathBuf> {
        let bin_name = checked_bin_name(bin_name)?;
        let want_hex = expected_sha.strip_prefix("sha256:").unwrap_or(expected_sha);
        let actual = hex::encode(Sha256::digest(tempfile_bytes));
        if actual != want_hex {
            return Err(anyhow!(
                "sha mismatch — refusing to write to cache. expected={want_hex} actual={actual}"
            ));
        }
        self.migrate_legacy_entry(&actual, bin_name).await?;
        ensure_dir(&self.paths.cached_binary_dir(&actual)).await?;
        let dest = self.paths.cached_binary(&actual, bin_name);
        atomic_write_executable(&dest, tempfile_bytes).await?;
        Ok(dest)
    }

    /// Install an already-streamed tempfile (DATA-505).
    ///
    /// The sha was computed incrementally while the bytes were being written,
    /// so this verifies `computed_sha == expected_sha` and then `rename(2)`s
    /// the file into `bin/<sha>/<name>` — no second pass over the artifact, no
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
        bin_name: &str,
    ) -> Result<PathBuf> {
        let bin_name = checked_bin_name(bin_name)?;
        let want_hex = expected_sha.strip_prefix("sha256:").unwrap_or(expected_sha);
        let got_hex = computed_sha.strip_prefix("sha256:").unwrap_or(computed_sha);
        if got_hex != want_hex {
            fs::remove_file(temp_path).await.ok();
            return Err(anyhow!(
                "sha mismatch — refusing to write to cache. expected={want_hex} actual={got_hex}"
            ));
        }

        self.migrate_legacy_entry(got_hex, bin_name).await?;
        ensure_dir(&self.paths.cached_binary_dir(got_hex)).await?;
        let dest = self.paths.cached_binary(got_hex, bin_name);

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
    ///
    /// Understands both shapes: a `bin/<sha>` directory (every file inside it
    /// must hash to the directory name) and a legacy `bin/<sha>` file. A hash
    /// directory left empty by deletions is pruned so the cache doesn't
    /// accumulate hollow entries.
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
            let Some(sha) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // `.incoming.*` / `.migrating.*` scratch files are not entries.
            if sha.starts_with('.') {
                continue;
            }
            let is_dir = match fs::metadata(&path).await {
                Ok(m) => m.is_dir(),
                Err(_) => continue,
            };

            if !is_dir {
                // Legacy file entry. Verify in place; migration to
                // `<sha>/<name>` happens lazily on next run, when the name is
                // known.
                if hash_of(&path).await.is_some_and(|h| h != sha) {
                    fs::remove_file(&path).await.ok();
                    mismatched.push(path);
                }
                continue;
            }

            let mut names = match fs::read_dir(&path).await {
                Ok(n) => n,
                Err(_) => continue,
            };
            let mut survivors = 0usize;
            while let Some(bin) = names.next_entry().await? {
                let bin_path = bin.path();
                if bin_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'))
                {
                    continue;
                }
                match hash_of(&bin_path).await {
                    Some(h) if h == sha => survivors += 1,
                    Some(_) => {
                        fs::remove_file(&bin_path).await.ok();
                        mismatched.push(bin_path);
                    }
                    None => continue,
                }
            }
            if survivors == 0 {
                // Nothing valid left under this hash — drop the shell.
                fs::remove_dir(&path).await.ok();
            }
        }
        Ok(mismatched)
    }

    /// Bring a pre-DATA-510 `bin/<sha>` **file** into the `bin/<sha>/<name>`
    /// shape, in place and without re-downloading. No-op when the entry is
    /// absent or already a directory, so it is safe — and cheap — to call on
    /// every lookup and every install.
    ///
    /// The destination path is the one the legacy file currently occupies, so
    /// the swap is staged in a scratch *directory* beside it:
    ///
    /// 1. hard-link the legacy file into `.migrating.<sha>.<rand>/<name>`, so
    ///    the content exists in both places at once;
    /// 2. unlink the legacy file (the data survives via the second link);
    /// 3. rename the scratch directory onto the now-free `bin/<sha>`.
    ///
    /// Every step is a single atomic syscall, and every way one can fail means
    /// another process got there first: a directory can't be hard-linked or
    /// unlinked as a file, and a rename onto a populated directory is refused.
    /// So a lost race is detected rather than corrupting the store, and no
    /// lock file is needed. A crash between (2) and (3) strands the content in
    /// the scratch directory — the entry then simply misses and is refetched.
    async fn migrate_legacy_entry(&self, sha: &str, bin_name: &str) -> Result<()> {
        let legacy = self.paths.legacy_cached_binary_file(sha);
        match fs::metadata(&legacy).await {
            // Already migrated (or never existed in the old shape).
            Ok(m) if m.is_dir() => return Ok(()),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).with_context(|| format!("stat {}", legacy.display())),
        }

        let hex = sha.strip_prefix("sha256:").unwrap_or(sha);
        let staged = self
            .paths
            .binary_cache_dir()
            .join(format!(".migrating.{hex}.{:016x}", rand::random::<u64>()));
        ensure_dir(&staged).await?;
        let staged_bin = staged.join(bin_name);

        // 1. Duplicate the entry into the staging dir. A directory here means
        //    the store is already migrated, and hard_link refuses directories.
        if let Err(e) = fs::hard_link(&legacy, &staged_bin).await {
            fs::remove_dir_all(&staged).await.ok();
            if fs::metadata(&legacy)
                .await
                .map(|m| m.is_dir())
                .unwrap_or(true)
            {
                return Ok(());
            }
            // Some filesystems refuse hard links outright — fall back to a
            // copy, which costs the bytes once and is otherwise identical.
            ensure_dir(&staged).await?;
            if fs::copy(&legacy, &staged_bin).await.is_err() {
                fs::remove_dir_all(&staged).await.ok();
                return Err(e)
                    .with_context(|| format!("staging {} for migration", legacy.display()));
            }
        }

        // 2. Drop the old name. The content lives on through the staged link.
        if let Err(e) = fs::remove_file(&legacy).await {
            fs::remove_dir_all(&staged).await.ok();
            // Not a file any more ⇒ someone else migrated it under us.
            if e.kind() == std::io::ErrorKind::NotFound
                || fs::metadata(&legacy)
                    .await
                    .map(|m| m.is_dir())
                    .unwrap_or(false)
            {
                return Ok(());
            }
            return Err(e).with_context(|| format!("removing legacy {}", legacy.display()));
        }

        // 3. Move the finished directory into place. A populated directory at
        //    the destination is the winner of a race — keep theirs, drop ours.
        if let Err(e) = fs::rename(&staged, &legacy).await {
            fs::remove_dir_all(&staged).await.ok();
            if fs::metadata(&legacy)
                .await
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                return Ok(());
            }
            return Err(e).with_context(|| {
                format!("migrating {} -> {}", staged.display(), legacy.display())
            });
        }

        tracing::debug!(
            sha = hex,
            bin_name,
            "migrated cached binary to the <hash>/<name> layout"
        );
        Ok(())
    }
}

/// Reject a bin name that could escape its hash directory. Names come from
/// component/tool identifiers, which are already validated at publish time —
/// this is the defence-in-depth check at the point the name becomes a path.
fn checked_bin_name(name: &str) -> Result<&str> {
    validate_tool_name(name)
        .map_err(|e| anyhow!("refusing to use {name:?} as a cached binary name: {e:?}"))?;
    Ok(name)
}

/// Hash a file, or `None` if it can't be read (a directory, a racing delete).
async fn hash_of(path: &Path) -> Option<String> {
    fs::read(path)
        .await
        .ok()
        .map(|b| hex::encode(Sha256::digest(&b)))
}

/// Materialise `dest` from any existing name in the same hash directory —
/// identical bytes by construction, so a hard link is enough. Returns whether
/// `dest` now exists.
async fn link_from_sibling(dir: &Path, dest: &Path) -> Result<bool> {
    let mut entries = match fs::read_dir(dir).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("read_dir {}", dir.display())),
    };
    while let Some(entry) = entries.next_entry().await? {
        let src = entry.path();
        let Some(name) = src.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') || !entry.file_type().await.is_ok_and(|t| t.is_file()) {
            continue;
        }
        match fs::hard_link(&src, dest).await {
            Ok(()) => return Ok(true),
            // A concurrent run linked the same name first.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(true),
            // Filesystems that refuse hard links still get a working entry.
            Err(_) => {
                fs::copy(&src, dest)
                    .await
                    .with_context(|| format!("copying {} -> {}", src.display(), dest.display()))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = fs::metadata(dest).await?.permissions();
                    perms.set_mode(0o755);
                    fs::set_permissions(dest, perms).await?;
                }
                return Ok(true);
            }
        }
    }
    Ok(false)
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

    /// Write `bytes` at `bin/<sha>` as a plain executable file — the layout
    /// every cache written before DATA-510 has on disk.
    async fn legacy_entry(paths: &GlobalPaths, bytes: &[u8]) -> String {
        let sha = sha256_hex(bytes);
        ensure_dir(&paths.binary_cache_dir()).await.unwrap();
        atomic_write_executable(&paths.legacy_cached_binary_file(&sha), bytes)
            .await
            .unwrap();
        sha
    }

    async fn entry_names(paths: &GlobalPaths, sha: &str) -> Vec<String> {
        let mut rd = tokio::fs::read_dir(paths.cached_binary_dir(sha))
            .await
            .unwrap();
        let mut names = Vec::new();
        while let Some(e) = rd.next_entry().await.unwrap() {
            names.push(e.file_name().to_string_lossy().to_string());
        }
        names.sort();
        names
    }

    #[tokio::test]
    async fn read_by_sha_returns_none_when_absent() {
        let td = TempDir::new().unwrap();
        let c = BinaryCache::new(paths_under(&td));
        assert!(c.read_by_sha("abc", "hello").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn finalize_writes_and_read_by_sha_finds() {
        let td = TempDir::new().unwrap();
        let c = BinaryCache::new(paths_under(&td));
        let bytes = b"abc";
        let sha = sha256_hex(bytes);
        let written = c.finalize(bytes, &sha, "hello").await.unwrap();
        let found = c.read_by_sha(&sha, "hello").await.unwrap().unwrap();
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
                "hello",
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
        c.finalize(bytes, &prefixed, "hello").await.unwrap();
        // Cached under the hex-only hash dir:
        assert!(c.read_by_sha(&prefixed, "hello").await.unwrap().is_some());
        assert!(
            c.read_by_sha(&sha256_hex(bytes), "hello")
                .await
                .unwrap()
                .is_some()
        );
    }

    // --- `<hash>/<name>` layout (DATA-510) ---

    #[tokio::test]
    async fn finalize_installs_under_hash_dir_named_after_the_component() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"#!/bin/sh\necho hi\n";
        let sha = sha256_hex(bytes);

        let dest = c.finalize(bytes, &sha, "shiitake").await.unwrap();

        assert_eq!(dest, paths.cached_binary(&sha, "shiitake"));
        // The property the whole change exists for: exec'ing this path gives
        // the child an argv[0] whose basename is the tool name.
        assert_eq!(dest.file_name().unwrap(), "shiitake");
        assert_eq!(dest.parent().unwrap().file_name().unwrap(), sha.as_str());
        assert!(paths.cached_binary_dir(&sha).is_dir());
        assert_eq!(tokio::fs::read(&dest).await.unwrap(), bytes);
    }

    #[tokio::test]
    async fn installed_binary_is_executable() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let td = TempDir::new().unwrap();
            let c = BinaryCache::new(paths_under(&td));
            let bytes = b"#!/bin/sh\n";
            let sha = sha256_hex(bytes);
            let dest = c.finalize(bytes, &sha, "hello").await.unwrap();
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[tokio::test]
    async fn one_hash_can_hold_several_names() {
        // Two components whose artifacts hash identically (or one tool reached
        // under an alias) share the hash dir, each under its own name.
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"shared bytes";
        let sha = sha256_hex(bytes);

        c.finalize(bytes, &sha, "ripgrep").await.unwrap();
        c.finalize(bytes, &sha, "rg").await.unwrap();

        assert_eq!(entry_names(&paths, &sha).await, vec!["rg", "ripgrep"]);
        for name in ["rg", "ripgrep"] {
            let p = c.read_by_sha(&sha, name).await.unwrap().unwrap();
            assert_eq!(p.file_name().unwrap(), name);
            assert_eq!(tokio::fs::read(&p).await.unwrap(), bytes);
        }
    }

    #[tokio::test]
    async fn read_by_sha_materialises_a_new_name_from_an_existing_sibling() {
        // Same content already cached under another name ⇒ link it into place
        // rather than reporting a miss and re-downloading.
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"dedup me";
        let sha = sha256_hex(bytes);
        c.finalize(bytes, &sha, "first").await.unwrap();

        let p = c.read_by_sha(&sha, "second").await.unwrap().unwrap();

        assert_eq!(p, paths.cached_binary(&sha, "second"));
        assert_eq!(tokio::fs::read(&p).await.unwrap(), bytes);
        assert_eq!(entry_names(&paths, &sha).await, vec!["first", "second"]);
    }

    #[tokio::test]
    async fn read_by_sha_misses_when_the_hash_is_not_cached_at_all() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        c.finalize(b"one", &sha256_hex(b"one"), "one")
            .await
            .unwrap();
        // A different sha: no dir to link a sibling from.
        assert!(
            c.read_by_sha(&sha256_hex(b"two"), "two")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn rejects_a_bin_name_that_would_escape_the_hash_dir() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"payload";
        let sha = sha256_hex(bytes);

        for bad in ["../escape", "a/b", "..", ""] {
            assert!(
                c.finalize(bytes, &sha, bad).await.is_err(),
                "{bad:?} must not be usable as a cached binary name"
            );
            assert!(c.read_by_sha(&sha, bad).await.is_err(), "{bad:?}");
        }
        assert!(
            !paths.binary_cache_dir().join("escape").exists(),
            "nothing may be written outside the hash dir"
        );
    }

    // --- migration from the pre-DATA-510 file layout ---

    #[tokio::test]
    async fn migrates_a_legacy_file_entry_in_place_on_read() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"an already-installed tool";
        let sha = legacy_entry(&paths, bytes).await;
        assert!(
            paths.legacy_cached_binary_file(&sha).is_file(),
            "precondition: the old layout is a file"
        );

        let p = c.read_by_sha(&sha, "gitnow").await.unwrap().unwrap();

        assert_eq!(p, paths.cached_binary(&sha, "gitnow"));
        assert!(paths.cached_binary_dir(&sha).is_dir());
        assert_eq!(
            tokio::fs::read(&p).await.unwrap(),
            bytes,
            "the installed bytes must survive the move — no re-download"
        );
        assert_eq!(entry_names(&paths, &sha).await, vec!["gitnow"]);
    }

    #[tokio::test]
    async fn migration_preserves_the_executable_bit() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let td = TempDir::new().unwrap();
            let paths = paths_under(&td);
            let c = BinaryCache::new(paths.clone());
            let sha = legacy_entry(&paths, b"#!/bin/sh\necho hi\n").await;

            let p = c.read_by_sha(&sha, "hello").await.unwrap().unwrap();

            let mode = std::fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755, "must stay runnable after migrating");
        }
    }

    #[tokio::test]
    async fn migration_is_idempotent() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"repeatedly resolved";
        let sha = legacy_entry(&paths, bytes).await;

        let first = c.read_by_sha(&sha, "hello").await.unwrap().unwrap();
        let second = c.read_by_sha(&sha, "hello").await.unwrap().unwrap();
        let third = c.read_by_sha(&sha, "hello").await.unwrap().unwrap();

        assert_eq!(first, second);
        assert_eq!(second, third);
        assert_eq!(entry_names(&paths, &sha).await, vec!["hello"]);
        assert_eq!(tokio::fs::read(&third).await.unwrap(), bytes);
        // No scratch files left behind.
        let mut rd = tokio::fs::read_dir(paths.binary_cache_dir()).await.unwrap();
        let mut top = Vec::new();
        while let Some(e) = rd.next_entry().await.unwrap() {
            top.push(e.file_name().to_string_lossy().to_string());
        }
        assert_eq!(top, vec![sha]);
    }

    #[tokio::test]
    async fn migration_runs_on_install_too_so_a_legacy_entry_never_blocks_a_write() {
        // A legacy *file* sits exactly where the new hash *dir* must go — an
        // install that didn't migrate first would fail to create the dir.
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"same content, new layout";
        let sha = legacy_entry(&paths, bytes).await;

        let dest = c.finalize(bytes, &sha, "hello").await.unwrap();

        assert_eq!(dest, paths.cached_binary(&sha, "hello"));
        assert_eq!(tokio::fs::read(&dest).await.unwrap(), bytes);
    }

    #[tokio::test]
    async fn concurrent_migration_of_the_same_entry_converges() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"raced migration";
        let sha = legacy_entry(&paths, bytes).await;

        let (a, b, d) = tokio::join!(
            c.read_by_sha(&sha, "hello"),
            c.read_by_sha(&sha, "hello"),
            c.read_by_sha(&sha, "hello"),
        );

        for r in [a, b, d] {
            let p = r.unwrap().expect("every racer must resolve the binary");
            assert_eq!(p, paths.cached_binary(&sha, "hello"));
        }
        assert_eq!(
            tokio::fs::read(paths.cached_binary(&sha, "hello"))
                .await
                .unwrap(),
            bytes
        );
        assert_eq!(entry_names(&paths, &sha).await, vec!["hello"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parallel_processes_migrating_the_same_entry_never_corrupt_it() {
        // The real shape of the race: several `forest global run`s landing on
        // one not-yet-migrated entry at once, on separate threads, some of
        // them wanting different names for the same content.
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = std::sync::Arc::new(BinaryCache::new(paths.clone()));
        let bytes = b"contended entry";
        let sha = legacy_entry(&paths, bytes).await;

        let mut handles = Vec::new();
        for i in 0..12 {
            let c = std::sync::Arc::clone(&c);
            let sha = sha.clone();
            let name = if i % 3 == 0 { "alias" } else { "hello" };
            handles.push(tokio::spawn(async move {
                (name, c.read_by_sha(&sha, name).await)
            }));
        }

        for h in handles {
            let (name, res) = h.await.unwrap();
            let p = res
                .unwrap()
                .expect("the entry is on disk — must never miss");
            assert_eq!(p, paths.cached_binary(&sha, name));
            assert_eq!(
                tokio::fs::read(&p).await.unwrap(),
                bytes,
                "{name} must be the real binary, not a directory or a stub"
            );
        }
        assert_eq!(entry_names(&paths, &sha).await, vec!["alias", "hello"]);

        // The store settles on exactly one hash entry, no scratch left over.
        let mut rd = tokio::fs::read_dir(paths.binary_cache_dir()).await.unwrap();
        let mut top = Vec::new();
        while let Some(e) = rd.next_entry().await.unwrap() {
            top.push(e.file_name().to_string_lossy().to_string());
        }
        assert_eq!(top, vec![sha]);
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

        let dest = c
            .finalize_streamed(&tmp, &sha, &sha, "hello")
            .await
            .unwrap();
        assert_eq!(dest, paths.cached_binary(&sha, "hello"));
        assert_eq!(c.read_by_sha(&sha, "hello").await.unwrap().unwrap(), dest);
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
            let dest = c
                .finalize_streamed(&tmp, &sha, &sha, "hello")
                .await
                .unwrap();
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[tokio::test]
    async fn finalize_streamed_migrates_a_legacy_entry_before_installing() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"streamed over a legacy entry";
        let sha = legacy_entry(&paths, bytes).await;
        let tmp = streamed_temp(&paths, bytes).await;

        let dest = c
            .finalize_streamed(&tmp, &sha, &sha, "hello")
            .await
            .unwrap();

        assert_eq!(dest, paths.cached_binary(&sha, "hello"));
        assert_eq!(tokio::fs::read(&dest).await.unwrap(), bytes);
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
            .finalize_streamed(&tmp, &actual, &expected, "hello")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("sha mismatch"), "{err}");
        assert!(
            c.read_by_sha(&expected, "hello").await.unwrap().is_none(),
            "a mismatched download must never become a cache entry"
        );
        assert!(
            c.read_by_sha(&actual, "hello").await.unwrap().is_none(),
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
        c.finalize_streamed(&tmp, &sha, &format!("sha256:{sha}"), "hello")
            .await
            .unwrap();
        assert!(c.read_by_sha(&sha, "hello").await.unwrap().is_some());
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
            c.finalize_streamed(&a, &sha, &sha, "hello"),
            c.finalize_streamed(&b, &sha, &sha, "hello")
        );
        assert_eq!(ra.unwrap(), rb.unwrap());
        assert_eq!(
            tokio::fs::read(paths.cached_binary(&sha, "hello"))
                .await
                .unwrap(),
            bytes
        );

        let mut entries = tokio::fs::read_dir(paths.binary_cache_dir()).await.unwrap();
        let mut names = Vec::new();
        while let Some(e) = entries.next_entry().await.unwrap() {
            names.push(e.file_name().to_string_lossy().to_string());
        }
        assert_eq!(
            names,
            vec![sha.clone()],
            "no stray tempfiles, exactly one entry"
        );
        assert_eq!(entry_names(&paths, &sha).await, vec!["hello"]);
    }

    // --- re_verify ---

    #[tokio::test]
    async fn re_verify_deletes_mismatched_entries() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"hello";
        let sha = sha256_hex(bytes);
        c.finalize(bytes, &sha, "hello").await.unwrap();

        // Corrupt the cached binary.
        tokio::fs::write(paths.cached_binary(&sha, "hello"), b"tampered")
            .await
            .unwrap();

        let deleted = c.re_verify().await.unwrap();
        assert_eq!(deleted.len(), 1);
        assert!(c.read_by_sha(&sha, "hello").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn re_verify_prunes_a_hash_dir_left_empty() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let sha = sha256_hex(b"hello");
        c.finalize(b"hello", &sha, "hello").await.unwrap();
        tokio::fs::write(paths.cached_binary(&sha, "hello"), b"tampered")
            .await
            .unwrap();

        c.re_verify().await.unwrap();

        assert!(
            !paths.cached_binary_dir(&sha).exists(),
            "an emptied hash dir must not linger"
        );
    }

    #[tokio::test]
    async fn re_verify_keeps_good_entries_and_only_drops_the_bad_name() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let bytes = b"good content";
        let sha = sha256_hex(bytes);
        c.finalize(bytes, &sha, "keep").await.unwrap();
        // A second name under the same hash, corrupted.
        tokio::fs::write(paths.cached_binary(&sha, "drop"), b"tampered")
            .await
            .unwrap();

        let deleted = c.re_verify().await.unwrap();

        assert_eq!(deleted, vec![paths.cached_binary(&sha, "drop")]);
        assert_eq!(entry_names(&paths, &sha).await, vec!["keep"]);
    }

    #[tokio::test]
    async fn re_verify_still_understands_the_legacy_file_layout() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let good = legacy_entry(&paths, b"intact").await;
        let bad = legacy_entry(&paths, b"will be corrupted").await;
        tokio::fs::write(paths.legacy_cached_binary_file(&bad), b"tampered")
            .await
            .unwrap();

        let deleted = c.re_verify().await.unwrap();

        assert_eq!(deleted, vec![paths.legacy_cached_binary_file(&bad)]);
        assert!(
            paths.legacy_cached_binary_file(&good).is_file(),
            "an intact legacy entry must survive verify untouched"
        );
    }

    #[tokio::test]
    async fn re_verify_ignores_in_flight_scratch_files() {
        let td = TempDir::new().unwrap();
        let paths = paths_under(&td);
        let c = BinaryCache::new(paths.clone());
        let tmp = streamed_temp(&paths, b"a download in progress").await;

        let deleted = c.re_verify().await.unwrap();

        assert!(deleted.is_empty());
        assert!(tmp.exists(), "a concurrent download must not be swept away");
    }
}
