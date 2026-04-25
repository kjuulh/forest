//! `forest-contrib/git-commit-push@v0.1` — stage, commit, push.
//!
//! Idempotent against partially-set-up repos: if `repo` isn't a git
//! working tree we `git init`; if there's no `origin` we add it; if the
//! requested branch doesn't exist we create it via `git checkout -B`.
//! Author identity is configured locally on the repo (not globally).

#[allow(dead_code)]
mod forestgen;

use std::path::Path;
use std::process::Command;

use forestgen::*;

struct Commands;

impl CommandHandler for Commands {
    async fn git_commit_push(
        &self,
        _spec: &Spec,
        input: GitCommitPushInput,
    ) -> Result<GitCommitPushOutput, forest_sdk::Error> {
        let repo = &input.repo;

        if !Path::new(repo).join(".git").exists() {
            git(repo, &["init", "-q", "-b", &input.branch])?;
        }

        git(repo, &["config", "user.name", &input.user_name])?;
        git(repo, &["config", "user.email", &input.user_email])?;

        // Make sure we're on the requested branch (creates if absent).
        git(repo, &["checkout", "-q", "-B", &input.branch])?;

        // Wire `origin` to the requested remote (idempotent: replace if
        // already set so re-runs against a different remote work).
        let _ = git(repo, &["remote", "remove", "origin"]);
        git(repo, &["remote", "add", "origin", &input.remote_url])?;

        git(repo, &["add", "-A"])?;

        let mut commit_args = vec!["commit", "-q", "-m", &input.message];
        if input.allow_empty {
            commit_args.push("--allow-empty");
        }
        git(repo, &commit_args)?;

        git(repo, &["push", "origin", &input.branch])?;

        let commit_sha = git_output(repo, &["rev-parse", "HEAD"])?;

        Ok(GitCommitPushOutput {
            commit_sha,
            pushed_branch: input.branch,
            remote_url: input.remote_url,
        })
    }
}

fn git(cwd: &str, args: &[&str]) -> Result<(), forest_sdk::Error> {
    let status = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .map_err(|e| forest_sdk::Error::Handler(format!("spawn git {args:?}: {e}").into()))?;
    if !status.success() {
        return Err(forest_sdk::Error::Handler(
            format!("git {args:?} failed: {status}").into(),
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
                String::from_utf8_lossy(&out.stderr)
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
