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

        git(repo, &["checkout", "-q", "-B", &input.branch])?;

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
