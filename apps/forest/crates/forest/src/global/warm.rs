use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use crate::global::paths::GlobalPaths;

pub const EXIT_NOT_CACHED: i32 = 75;

pub const DISABLE_ENV: &str = "FOREST_NO_GLOBAL_WARM";
pub const INTERVAL_ENV: &str = "FOREST_GLOBAL_WARM_INTERVAL_SECS";
pub const CHILD_GUARD_ENV: &str = "FOREST_GLOBAL_WARM_CHILD";
pub const NO_FETCH_ENV: &str = "FOREST_GLOBAL_NO_FETCH";

const DEFAULT_INTERVAL: Duration = Duration::from_secs(30 * 60);

const LOCK_STALE_AFTER: Duration = Duration::from_secs(30 * 60);

fn stamp_path(paths: &GlobalPaths) -> PathBuf {
    paths.state_dir().join("global-warm.stamp")
}

fn lock_path(paths: &GlobalPaths) -> PathBuf {
    paths.state_dir().join("global-warm.lock")
}

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

fn age_of(path: &Path) -> Option<Duration> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    Some(
        SystemTime::now()
            .duration_since(mtime)
            .unwrap_or(Duration::ZERO),
    )
}

fn claim_slot(stamp: &Path, interval: Duration) -> bool {
    if let Some(parent) = stamp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if let Some(age) = age_of(stamp)
        && age < interval
    {
        return false;
    }

    if age_of(stamp).is_some() {
        let _ = std::fs::remove_file(stamp);
    }

    let staging = stamp.with_extension("stamp.new");
    let _ = std::fs::remove_file(&staging);
    if std::fs::write(&staging, chrono::Utc::now().to_rfc3339().as_bytes()).is_err() {
        return false;
    }
    let won = std::fs::hard_link(&staging, stamp).is_ok();
    let _ = std::fs::remove_file(&staging);
    won
}

pub struct WarmLock {
    path: PathBuf,
}

impl WarmLock {
    pub fn acquire(paths: &GlobalPaths) -> Option<Self> {
        Self::acquire_with(paths, LOCK_STALE_AFTER)
    }

    fn acquire_with(paths: &GlobalPaths, stale_after: Duration) -> Option<Self> {
        let path = lock_path(paths);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if Self::create(&path) {
            return Some(Self { path });
        }
        match age_of(&path) {
            Some(age) if age >= stale_after => {
                let _ = std::fs::remove_file(&path);
                Self::create(&path).then_some(Self { path })
            }
            _ => None,
        }
    }

    fn create(path: &Path) -> bool {
        use std::io::Write;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut f) => {
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

pub fn maybe_spawn(paths: &GlobalPaths) -> bool {
    if disabled() {
        return false;
    }
    if !claim_slot(&stamp_path(paths), interval()) {
        return false;
    }
    spawn_detached()
}

pub fn spawn_detached() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["global", "warm", "--quiet"])
        .env(CHILD_GUARD_ENV, "1")
        .env_remove(NO_FETCH_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

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
        assert!(claim_slot(&stamp, Duration::ZERO));
    }

    #[test]
    fn claim_leaves_no_staging_file_behind() {
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
        let (_d, paths) = tmp_paths();
        assert_ne!(stamp_path(&paths), lock_path(&paths));
    }

    #[test]
    fn exit_code_is_the_sysexits_tempfail_value() {
        assert_eq!(EXIT_NOT_CACHED, 75);
    }

    #[test]
    fn warm_interval_is_shorter_than_the_daily_auto_update() {
        assert!(DEFAULT_INTERVAL < Duration::from_secs(24 * 60 * 60));
    }

    #[test]
    fn child_guard_disables_further_warming() {
        unsafe { std::env::set_var(CHILD_GUARD_ENV, "1") };
        let d = disabled();
        unsafe { std::env::remove_var(CHILD_GUARD_ENV) };
        assert!(d);
    }
}
