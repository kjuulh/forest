#[allow(dead_code)]
mod forestgen;

use std::process::Command;

use forestgen::*;

struct Commands;

impl CommandHandler for Commands {
    async fn checkout(
        &self,
        _spec: &Spec,
        input: CheckoutInput,
    ) -> Result<CheckoutOutput, forest_sdk::Error> {
        let mut clone = Command::new("git");
        clone.arg("clone");
        if input.depth > 0 {
            clone.arg(format!("--depth={}", input.depth));
            clone.arg("--single-branch");
        }
        if let Some(r) = &input.r#ref {
            clone.args(["--branch", r]);
        }
        clone.arg(&input.repo).arg(&input.dest);

        let out = clone
            .output()
            .map_err(|e| forest_sdk::Error::Handler(format!("spawn git clone: {e}").into()))?;
        if !out.status.success() {
            return Err(forest_sdk::Error::Handler(
                format!(
                    "git clone exited with {} — {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                )
                .into(),
            ));
        }
        eprintln!("{}", String::from_utf8_lossy(&out.stderr).trim_end());

        let commit_sha = git_output(&input.dest, &["rev-parse", "HEAD"])?;
        let branch = git_output(&input.dest, &["rev-parse", "--abbrev-ref", "HEAD"])?;

        Ok(CheckoutOutput {
            commit_sha,
            branch,
            dest: input.dest,
        })
    }
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
