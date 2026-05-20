# forest-contrib/git-commit-push

Stage everything, commit, push to `origin`. The terminal step in the
typical "scaffold → init → commit → push" chain.

## Inputs

- `repo` — path to the git working tree.
- `branch` — branch to push to.
- `remote_url` — the remote `origin` should point at.
- `user_name`, `user_email` — author identity (local to the repo).
- `message` — commit message.
- `allow_empty` — pass through to `git commit --allow-empty`. Useful
  when the workflow re-runs and there's nothing new to commit.

## Behaviour (all idempotent)

- If `repo` isn't a git working tree → `git init`.
- Local user identity is always re-set so a re-run with different
  credentials doesn't silently use stale values.
- `origin` is removed and re-added every run so changing the remote
  between runs Just Works.
- The branch is created (or switched to) via `git checkout -B`.
- Everything is staged with `git add -A`, then committed and pushed.

## Output

`commit_sha`, `branch`, `pushed_to`.

## Auth

We don't manage credentials. Provide them out-of-band — SSH keys on
the runner, an `https://user:token@host/...` URL passed as
`remote_url`, or a git credential helper configured in the runner
environment.
