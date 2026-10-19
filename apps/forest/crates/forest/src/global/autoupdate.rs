use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use crate::global::paths::GlobalPaths;

const DISABLE_ENV: &str = "FOREST_NO_AUTO_UPDATE";
const INTERVAL_ENV: &str = "FOREST_AUTO_UPDATE_INTERVAL_SECS";
const CHILD_GUARD_ENV: &str = "FOREST_AUTO_UPDATE_CHILD";

const DEFAULT_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

fn stamp_path(paths: &GlobalPaths) -> PathBuf {
    paths.state_dir().join("global-autoupdate.stamp")
}

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

fn is_due(stamp: &Path, interval: Duration) -> bool {
    match std::fs::metadata(stamp).and_then(|m| m.modified()) {
        Ok(mtime) => match SystemTime::now().duration_since(mtime) {
            Ok(elapsed) => elapsed >= interval,
            Err(_) => false,
        },
        Err(_) => true,
    }
}

fn claim(stamp: &Path) -> bool {
    if let Some(parent) = stamp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(stamp, chrono::Utc::now().to_rfc3339().as_bytes()).is_ok()
}

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

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let _ = cmd.spawn();
}

pub fn maybe_spawn(paths: &GlobalPaths) {
    if should_skip() {
        return;
    }
    let stamp = stamp_path(paths);
    if !is_due(&stamp, interval()) {
        return;
    }
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
        assert!(is_due(&stamp, Duration::from_secs(0)));
        let _ = std::fs::remove_file(&stamp);
    }

    #[test]
    fn interval_defaults_when_env_unset() {
        assert_eq!(super::DEFAULT_INTERVAL, Duration::from_secs(24 * 60 * 60));
    }
}
