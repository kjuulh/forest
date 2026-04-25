//! `forest-contrib/git-init@v0.1` — initialise a fresh git repository
//! at `work_dir` with a configured author identity and an empty initial
//! commit. Idempotent: if work_dir is already a git repo, leaves it
//! alone and reports the current HEAD.

#[allow(dead_code)]
mod forestgen;

use std::path::Path;
use std::process::Command;

use forestgen::*;

struct Commands;

impl CommandHandler for Commands {
    async fn git_init(
        &self,
        _spec: &Spec,
        input: GitInitInput,
        context: &forest_sdk::CallContext,
    ) -> Result<GitInitOutput, forest_sdk::Error> {
        let work_dir = context
            .work_dir
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(".");

        let already = Path::new(work_dir).join(".git").exists();
        if already {
            // Read back what's already there so the workflow author
            // gets consistent outputs regardless of whether we
            // initialised or no-op'd.
            let sha = git_output(work_dir, &["rev-parse", "HEAD"])
                .unwrap_or_else(|_| String::new());
            let branch = git_output(work_dir, &["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap_or_else(|_| input.branch.clone());
            return Ok(GitInitOutput {
                branch,
                initial_commit_sha: sha,
                already_initialized: true,
            });
        }

        git(work_dir, &["init", "-q", "-b", &input.branch])?;
        git(work_dir, &["config", "user.name", &input.user_name])?;
        git(work_dir, &["config", "user.email", &input.user_email])?;
        git(work_dir, &["commit", "--allow-empty", "-q", "-m", &input.message])?;

        let sha = git_output(work_dir, &["rev-parse", "HEAD"])?;
        Ok(GitInitOutput {
            branch: input.branch,
            initial_commit_sha: sha,
            already_initialized: false,
        })
    }
}

fn git(cwd: &str, args: &[&str]) -> Result<(), forest_sdk::Error> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|e| forest_sdk::Error::Handler(format!("spawn git {args:?}: {e}").into()))?;
    if !out.status.success() {
        return Err(forest_sdk::Error::Handler(
            format!(
                "git {args:?} failed (exit {:?}): {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            )
            .into(),
        ));
    }
    Ok(())
}

fn git_output(cwd: &str, args: &[&str]) -> Result<String, forest_sdk::Error> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|e| forest_sdk::Error::Handler(format!("spawn git {args:?}: {e}").into()))?;
    if !out.status.success() {
        return Err(forest_sdk::Error::Handler(
            format!(
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )
            .into(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    let router = ComponentRouter::new(Commands);
    forest_sdk::run_once(&router);
}
