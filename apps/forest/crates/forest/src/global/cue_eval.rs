//! Evaluates CUE files to JSON by shelling out to the `cue` binary (Q6.a).
//!
//! Per-process memoisation by (path, mtime, size); no on-disk cache yet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow};
use tokio::process::Command;

#[derive(Default)]
pub struct CueEvaluator {
    memo: Mutex<HashMap<MemoKey, String>>,
}

#[derive(Hash, Eq, PartialEq)]
struct MemoKey {
    path: PathBuf,
    mtime_nanos: i128,
    size: u64,
}

impl CueEvaluator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Evaluate the CUE package containing `path` and return its JSON form.
    ///
    /// Invocation: `cue eval --out json <path>` (so individual `.cue` files
    /// in the same package merge correctly). The caller picks which `.cue`
    /// entry-point to evaluate; we just pass it through.
    pub async fn eval_to_json(&self, path: &Path) -> Result<String> {
        let meta = tokio::fs::metadata(path)
            .await
            .with_context(|| format!("stat {}", path.display()))?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i128)
            .unwrap_or(0);
        let key = MemoKey {
            path: path.to_path_buf(),
            mtime_nanos: mtime,
            size: meta.len(),
        };

        if let Some(cached) = self.memo.lock().unwrap().get(&key) {
            return Ok(cached.clone());
        }

        let output = crate::tools::cue::output(|| {
            let mut cmd = Command::new("cue");
            cmd.arg("eval").arg("--out=json").arg(path);
            cmd
        })
        .await
        .with_context(|| format!("running `cue eval --out=json {}`", path.display()))?;

        if !output.status.success() {
            return Err(anyhow!(
                "cue eval failed for {}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        let json = String::from_utf8(output.stdout)
            .with_context(|| format!("cue stdout for {} not UTF-8", path.display()))?;

        self.memo.lock().unwrap().insert(key, json.clone());
        Ok(json)
    }
}
