# forest-contrib/git-init

Initialise a fresh git repository at the workflow's `work_dir`. The
typical successor to `forest-contrib/init`: turn the scaffolded files
into a real repo so the next step (commit-push) has something to push.

## Inputs

- `branch` — initial branch name (e.g. `main`).
- `user_name`, `user_email` — author identity, configured **locally**
  on the repo (we don't touch the user's global git config).
- `message` — initial commit message.

## Behaviour

- If `work_dir` is already a git repo: leave it alone and report the
  current HEAD / branch in the output. Lets re-runs of the workflow
  stay idempotent.
- Otherwise: `git init -b <branch>`, set the local identity, create an
  empty initial commit with `--allow-empty` so the branch exists.

The empty commit means downstream `git push` works on the first run
even before any project files have been staged.

## Output

`branch`, `initial_commit_sha`, `already_initialized` (so the workflow
can branch on first-run vs subsequent-run behaviour).
