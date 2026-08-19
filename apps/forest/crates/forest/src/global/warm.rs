//! Background prefetch ("warm") of global-tool binaries — DATA-588.
//!
//! Global tools install lazily: the shim runs `forest global run`, which
//! downloads the binary on first use. That is the right trade for `gitnow
//! status`, but it is the wrong trade for a shell rc file, which invokes
//! several tools purely to `eval` their shell integration:
//!
//! ```zsh
//! eval "$(gitnow init zsh)"
//! eval "$(awslogin shell zsh)"
//! ```
//!
//! On a cold cache each of those blocks a new shell on a multi-MB download,
//! serially, before the prompt appears. This module is the other half of the
//! fix: `forest global run --no-fetch` (used by the `forest-init` shell
//! helper) *skips* an uncached tool instead of downloading it, and asks this
//! module to pull the binaries down out-of-band instead.
//!
//! Two independent guards keep that background work from becoming a problem
//! of its own — a shell rc runs on every new terminal, so "spawns a process
//! each time" and "re-downloads each time" are both unacceptable:
//!
//! 1. **A throttle stamp** ([`claim_slot`]) — at most one warm is *started*
//!    per interval. The slot is claimed via [`std::fs::hard_link`], which
//!    fails if the destination exists and so acts as an atomic test-and-set
//!    on POSIX; a burst of shells opening at once therefore yields one warm,
//!    not one per shell.
//! 2. **A single-instance lock** ([`WarmLock`]) — held for the lifetime of
//!    the warm run itself, so even a `--force`d warm (or a lost race on the
//!    stamp) can never leave two downloaders competing for the same cache.
//!    Stale locks (from a killed warm) expire by mtime.
//!
//! Everything here is best-effort. A failure to spawn, stat, lock, or write
//! must never disturb the foreground command — worst case, the tool simply
//! stays cold and the next shell tries again.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use crate::global::paths::GlobalPaths;

/// Exit code `forest global run --no-fetch` uses to say "this tool isn't
/// cached yet, and I didn't download it". `75` is `EX_TEMPFAIL` from
/// `sysexits.h` — "the request failed, try again later", which is exactly the
/// contract: a background warm has been started, so a later attempt will
/// succeed. The `forest-init` shell helper treats this code as "queue for
/// retry", and anything else as a real failure it should not retry.
pub const EXIT_NOT_CACHED: i32 = 75;

/// Set to any value to disable background warming entirely. Presence-based,
/// matching the sibling `FOREST_NO_AUTO_UPDATE` / `FOREST_NO_UPDATE_CHECK`
/// opt-outs.
pub const DISABLE_ENV: &str = "FOREST_NO_GLOBAL_WARM";
/// Override the throttle interval, in seconds. Defaults to [`DEFAULT_INTERVAL`].
pub const INTERVAL_ENV: &str = "FOREST_GLOBAL_WARM_INTERVAL_SECS";
/// Set on the spawned child so a warm never triggers another warm.
pub const CHILD_GUARD_ENV: &str = "FOREST_GLOBAL_WARM_CHILD";
/// Presence makes `forest global run` skip cold fetches (see [`EXIT_NOT_CACHED`]).
pub const NO_FETCH_ENV: &str = "FOREST_GLOBAL_NO_FETCH";

/// Default throttle: start at most one warm per 30 minutes.
///
/// Shorter than the daily auto-update interval on purpose. A warm is how a
/// *missing* tool becomes available, so an interval measured in hours would
/// mean a failed download (offline, expired auth) leaves the toolset cold for
/// the rest of the day. Half an hour is long enough that a burst of terminals
/// costs one warm, short enough that a transient failure heals on its own.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// A held lock older than this is assumed to belong to a warm that was killed
/// (a reboot mid-download, a `kill -9`) and is stolen. Generous enough to
/// cover a slow download of a large toolset over a bad connection.
const LOCK_STALE_AFTER: Duration = Duration::from_secs(30 * 60);

fn stamp_path(paths: &GlobalPaths) -> PathBuf {
    paths.state_dir().join("global-warm.stamp")
}

fn lock_path(paths: &GlobalPaths) -> PathBuf {
    paths.state_dir().join("global-warm.lock")
}

/// `true` when the user (or the environment) has opted out of background
/// warming, or when we are already inside a warm.
///
/// Deliberately *not* gated on a TTY, unlike [`crate::global::autoupdate`]:
/// auto-update is a nice-to-have that only interactive users should pay for,
/// whereas a warm is what makes a tool exist at all. A `zsh -ic` spawned by an
/// editor or by Claude Code has no TTY and still needs its tools.
pub fn disabled() -> bool {
    std::env::var_os(DISABLE_ENV).is_some() || std::env::var_os(CHILD_GUARD_ENV).is_some()
}

fn interval() -> Duration {
    std::env::var(INTERVAL_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_INTERVAL)
}

/// Age of `path`, or `None` if it doesn't exist / can't be read.
fn age_of(path: &Path) -> Option<Duration> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    // A timestamp in the future (clock skew, a restored backup) reads as brand
    // new rather than infinitely old — better to skip a warm than to spam one.
    Some(
        SystemTime::now()
            .duration_since(mtime)
            .unwrap_or(Duration::ZERO),
    )
}

/// Atomically claim this interval's warm slot.
///
/// Returns `true` at most once per `interval` across all concurrent forest
/// processes. The claim is a [`std::fs::hard_link`] onto the stamp path:
/// `link(2)` fails with `EEXIST` if the destination exists, giving us a
/// test-and-set that a plain `write` (which always succeeds, so two racing
/// shells would both "win") cannot.
fn claim_slot(stamp: &Path, interval: Duration) -> bool {
    if let Some(parent) = stamp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Not due yet → nothing to claim. Cheap path: one stat.
    if let Some(age) = age_of(stamp)
        && age < interval
    {
        return false;
    }

    // Due (or no stamp at all). Drop any expired stamp so the link below has
    // an empty slot to take. Losing this race is harmless: the loser's link
    // fails and it doesn't spawn.
    if age_of(stamp).is_some() {
        let _ = std::fs::remove_file(stamp);
    }

    // Stage the new stamp beside the target (same filesystem, so `link` can
    // work) and test-and-set it into place. Content is for humans reading the
    // file; the due-check only looks at mtime.
    let staging = stamp.with_extension("stamp.new");
    let _ = std::fs::remove_file(&staging);
    if std::fs::write(&staging, chrono::Utc::now().to_rfc3339().as_bytes()).is_err() {
        // Unwritable state dir. Returning false means we never spawn, which
        // is the safe direction — a "claim" we can't record would re-fire on
        // every single shell start.
        return false;
    }
    let won = std::fs::hard_link(&staging, stamp).is_ok();
    let _ = std::fs::remove_file(&staging);
    won
}

/// A held single-instance lock on the warm run. Removed on drop.
///
/// Coarser than the throttle stamp and serving a different purpose: the stamp
/// rate-limits *starting* warms, this guarantees at most one *running* warm.
/// Both are needed — `--force` and `--no-throttle` bypass the stamp on
/// purpose, and only this stops those from stacking downloads.
pub struct WarmLock {
    path: PathBuf,
}

impl WarmLock {
    /// Take the lock, or return `None` if another warm holds it.
    ///
    /// A lock older than [`LOCK_STALE_AFTER`] is stolen: its owner is gone
    /// (there is no pid liveness check by design — that would need a libc
    /// dependency, and an over-long download is indistinguishable from a
    /// crash anyway, so age is the honest signal).
    pub fn acquire(paths: &GlobalPaths) -> Option<Self> {
        Self::acquire_with(paths, LOCK_STALE_AFTER)
    }

    /// [`Self::acquire`] with an explicit staleness horizon. Split out so the
    /// steal-a-dead-lock path is testable without backdating file mtimes.
    fn acquire_with(paths: &GlobalPaths, stale_after: Duration) -> Option<Self> {
        let path = lock_path(paths);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if Self::create(&path) {
            return Some(Self { path });
        }
        // Held. Stale enough to steal?
        match age_of(&path) {
            Some(age) if age >= stale_after => {
                let _ = std::fs::remove_file(&path);
                Self::create(&path).then_some(Self { path })
            }
            _ => None,
        }
    }

    /// `create_new` is `O_EXCL`: it fails rather than truncating an existing
    /// lock, which is what makes this mutually exclusive.
    fn create(path: &Path) -> bool {
        use std::io::Write;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut f) => {
                // pid is diagnostic only — nothing reads it back.
                let _ = writeln!(f, "{}", std::process::id());
                true
            }
            Err(_) => false,
        }
    }
}

impl Drop for WarmLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Spawn a fully detached, silent `forest global warm --quiet`, subject to the
/// throttle.
///
/// Returns whether a warm was actually started. Safe (and cheap — one stat in
/// the common case) to call from any hot path, including once per uncached
/// tool during shell init: the throttle collapses a whole rc file's worth of
/// misses into a single background run.
pub fn maybe_spawn(paths: &GlobalPaths) -> bool {
    if disabled() {
        return false;
    }
    // Claim before spawning, exactly as `autoupdate` does: a slow or failing
    // warm must not turn into a thundering herd, and an unrecordable claim
    // must not spawn at all.
    if !claim_slot(&stamp_path(paths), interval()) {
        return false;
    }
    spawn_detached()
}

/// Spawn the detached warm child, bypassing the throttle. Callers that want
/// throttling should use [`maybe_spawn`].
pub fn spawn_detached() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["global", "warm", "--quiet"])
        .env(CHILD_GUARD_ENV, "1")
        // The child must never inherit "don't fetch" from the shell-init
        // wrapper that triggered it — fetching is its entire job.
        .env_remove(NO_FETCH_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Detach from the foreground process group so a Ctrl-C aimed at the shell
    // doesn't kill the warm, and so it outlives the shell that started it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    cmd.spawn().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_paths() -> (tempfile::TempDir, GlobalPaths) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let paths =
            GlobalPaths::with_roots(root.join("cfg"), root.join("state"), root.join("cache"));
        (dir, paths)
    }

    #[test]
    fn first_claim_wins_and_second_is_throttled() {
        let (_d, paths) = tmp_paths();
        let stamp = stamp_path(&paths);
        assert!(claim_slot(&stamp, DEFAULT_INTERVAL), "first claim");
        assert!(
            !claim_slot(&stamp, DEFAULT_INTERVAL),
            "second claim inside the interval must lose"
        );
    }

    #[test]
    fn claim_is_available_again_once_the_interval_elapses() {
        let (_d, paths) = tmp_paths();
        let stamp = stamp_path(&paths);
        assert!(claim_slot(&stamp, DEFAULT_INTERVAL));
        // A zero interval means "always due".
        assert!(claim_slot(&stamp, Duration::ZERO));
    }

    #[test]
    fn claim_leaves_no_staging_file_behind() {
        // The staging file is an implementation detail; leaking it into the
        // state dir would be visible litter (and would break the next claim's
        // `write` if it were ever left read-only).
        let (_d, paths) = tmp_paths();
        let stamp = stamp_path(&paths);
        assert!(claim_slot(&stamp, DEFAULT_INTERVAL));
        assert!(!stamp.with_extension("stamp.new").exists());
    }

    #[test]
    fn claim_creates_the_state_dir_on_first_run() {
        let (_d, paths) = tmp_paths();
        assert!(!paths.state_dir().exists());
        assert!(claim_slot(&stamp_path(&paths), DEFAULT_INTERVAL));
        assert!(stamp_path(&paths).exists());
    }

    #[test]
    fn lock_is_mutually_exclusive_while_held() {
        let (_d, paths) = tmp_paths();
        let held = WarmLock::acquire(&paths).expect("first acquire");
        assert!(
            WarmLock::acquire(&paths).is_none(),
            "a second warm must not run concurrently"
        );
        drop(held);
        assert!(
            WarmLock::acquire(&paths).is_some(),
            "lock must be released on drop"
        );
    }

    #[test]
    fn stale_lock_is_stolen() {
        let (_d, paths) = tmp_paths();
        let lock = lock_path(&paths);
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        std::fs::write(&lock, b"99999\n").unwrap();
        // Zero horizon: every existing lock is past it, i.e. its owner is
        // assumed gone. A lock left behind by a killed warm must not wedge
        // warming forever.
        assert!(
            WarmLock::acquire_with(&paths, Duration::ZERO).is_some(),
            "a lock from a dead warm must not block forever"
        );
    }

    #[test]
    fn fresh_foreign_lock_is_respected() {
        let (_d, paths) = tmp_paths();
        let lock = lock_path(&paths);
        std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
        std::fs::write(&lock, b"99999\n").unwrap();
        assert!(WarmLock::acquire(&paths).is_none());
    }

    #[test]
    fn stamp_and_lock_are_distinct_files() {
        // They guard different things (starting vs running); sharing a path
        // would make a running warm suppress the next interval's claim.
        let (_d, paths) = tmp_paths();
        assert_ne!(stamp_path(&paths), lock_path(&paths));
    }

    #[test]
    fn exit_code_is_the_sysexits_tempfail_value() {
        // The shell helper hard-codes 75; keep the two in sync.
        assert_eq!(EXIT_NOT_CACHED, 75);
    }

    #[test]
    fn warm_interval_is_shorter_than_the_daily_auto_update() {
        // A cold tool becomes available only once a warm runs, so the warm
        // cadence has to be tighter than the "bump to latest" cadence.
        assert!(DEFAULT_INTERVAL < Duration::from_secs(24 * 60 * 60));
    }

    #[test]
    fn child_guard_disables_further_warming() {
        // The spawned warm sets CHILD_GUARD_ENV; `disabled()` reading it is
        // what stops a warm from recursively spawning warms.
        unsafe { std::env::set_var(CHILD_GUARD_ENV, "1") };
        let d = disabled();
        unsafe { std::env::remove_var(CHILD_GUARD_ENV) };
        assert!(d);
    }
}
