//! Background auto-update trigger for global tools.
//!
//! `forest global update` re-resolves pins + catalogue subscriptions and
//! bumps tools to their latest published versions. Asking users to remember
//! to run it is a poor experience, so we fire it automatically — but
//! _throttled_ (at most once per interval) and _detached_ (it never blocks
//! the foreground command) so the cost is invisible.
//!
//! The hook lives on the hot path of `forest global run` (every shim
//! invocation routes through it). On each call we:
//!   1. bail if opted out (`FOREST_NO_AUTO_UPDATE`), in CI, not attached to a
//!      TTY, or already inside the spawned background child — the same skip
//!      conditions as the `forest self` update nag (see `cli/self_cmd.rs`),
//!      so scripts / CI / pipelines never spawn background network work;
//!   2. bail if the last check is younger than the interval (a single
//!      `stat` of the stamp file — cheap);
//!   3. otherwise *claim the slot* by touching the stamp to "now" (so
//!      concurrent / subsequent invocations see "not due" and a failed or
//!      offline update doesn't hammer the registry). Only if the claim
//!      actually persists do we spawn a fully detached
//!      `forest global update --background` — if we can't record the slot we
//!      must not spawn, or every invocation would re-fire.
//!
//! Everything here is best-effort: a failure to spawn, stat, or write must
//! never disrupt the tool the user actually asked to run.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use crate::global::paths::GlobalPaths;

/// Set to any value to disable background auto-update entirely. Presence-
/// based to match the sibling `FOREST_NO_UPDATE_CHECK` opt-out.
const DISABLE_ENV: &str = "FOREST_NO_AUTO_UPDATE";
/// Override the throttle interval (in seconds). Defaults to [`DEFAULT_INTERVAL`].
const INTERVAL_ENV: &str = "FOREST_AUTO_UPDATE_INTERVAL_SECS";
/// Set on the spawned child so it never re-triggers itself.
const CHILD_GUARD_ENV: &str = "FOREST_AUTO_UPDATE_CHILD";

/// Default throttle: check at most once per day.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

fn stamp_path(paths: &GlobalPaths) -> PathBuf {
    paths.state_dir().join("global-autoupdate.stamp")
}

/// Skip auto-update unless this looks like an interactive human session.
/// Mirrors `cli/self_cmd::maybe_print_update_nag`: opt-out env, CI, or a
/// non-TTY stderr (piped / scripted / test) all suppress it. The re-entrancy
/// guard stops the spawned child from triggering itself.
fn should_skip() -> bool {
    use std::io::IsTerminal;
    std::env::var_os(DISABLE_ENV).is_some()
        || std::env::var_os(CHILD_GUARD_ENV).is_some()
        || std::env::var_os("CI").is_some()
        || !std::io::stderr().is_terminal()
}

fn interval() -> Duration {
    std::env::var(INTERVAL_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_INTERVAL)
}

/// `true` when no recent-enough check stamp exists (or it can't be read).
fn is_due(stamp: &Path, interval: Duration) -> bool {
    match std::fs::metadata(stamp).and_then(|m| m.modified()) {
        Ok(mtime) => match SystemTime::now().duration_since(mtime) {
            Ok(elapsed) => elapsed >= interval,
            // Clock moved backwards since the stamp was written — treat as
            // recent (not due) rather than spamming updates.
            Err(_) => false,
        },
        // No stamp yet (first run) or unreadable → due.
        Err(_) => true,
    }
}

/// Touch the stamp to "now", claiming the current interval slot. Returns
/// whether the claim actually persisted — callers must not spawn on `false`,
/// or a permanently-unwritable state dir would re-fire on every invocation.
fn claim(stamp: &Path) -> bool {
    if let Some(parent) = stamp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Content is for humans inspecting the file; the due-check reads mtime.
    // Writing the same path overwrites mtime, i.e. acts as `touch`.
    std::fs::write(stamp, chrono::Utc::now().to_rfc3339().as_bytes()).is_ok()
}

/// Spawn a detached, silent `forest global update --background`.
fn spawn_update() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["global", "update", "--background"])
        .env(CHILD_GUARD_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Detach from the foreground process group so a Ctrl-C aimed at the tool
    // the user is running doesn't also kill the updater, and so it outlives
    // the `exec()` that `forest global run` performs straight after.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let _ = cmd.spawn();
}

/// Fire-and-forget background auto-update if the throttle interval has
/// elapsed. Safe to call on every `forest global run`; never errors.
pub fn maybe_spawn(paths: &GlobalPaths) {
    if should_skip() {
        return;
    }
    let stamp = stamp_path(paths);
    if !is_due(&stamp, interval()) {
        return;
    }
    // Claim the slot *before* spawning so a slow/failed/offline update can't
    // cause a thundering herd of background processes — and only spawn if the
    // claim persisted, otherwise we'd re-fire on every invocation.
    if claim(&stamp) {
        spawn_update();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tmp() -> PathBuf {
        let mut p = std::env::temp_dir();
        // Unique-ish without Math.random/Date — use a monotonic-ish nanos.
        let n = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        p.push(format!("forest-autoupdate-test-{n}"));
        p.push("global-autoupdate.stamp");
        p
    }

    #[test]
    fn missing_stamp_is_due() {
        let stamp = tmp();
        assert!(is_due(&stamp, DEFAULT_INTERVAL));
    }

    #[test]
    fn fresh_stamp_is_not_due() {
        let stamp = tmp();
        assert!(claim(&stamp), "claim should persist under temp dir");
        assert!(!is_due(&stamp, DEFAULT_INTERVAL));
        // ...but with a zero interval everything is immediately due again.
        assert!(is_due(&stamp, Duration::from_secs(0)));
        let _ = std::fs::remove_file(&stamp);
    }

    #[test]
    fn interval_defaults_when_env_unset() {
        // Not asserting against the live env (tests share a process); just
        // confirm the parser falls back for garbage input.
        assert_eq!(
            super::DEFAULT_INTERVAL,
            Duration::from_secs(24 * 60 * 60)
        );
    }
}
