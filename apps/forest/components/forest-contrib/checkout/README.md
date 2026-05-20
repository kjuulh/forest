# forest-contrib/checkout

A `git clone` wrapper shaped for Forest workflows. The spiritual
successor to GitHub Actions' `actions/checkout` — works against any
URL `git` itself understands (`https://`, `ssh://`, `file://`, local
path).

## Inputs

- `repo` — the URL or path to clone from.
- `dest` — where to clone into.
- `ref` *(optional)* — a branch or tag to check out instead of the
  default HEAD.
- `depth` *(optional)* — pass `--depth N --single-branch` for a
  shallow clone. `0` (default) = full history.

## Behaviour

- Shells out to the system `git` rather than embedding libgit2, so
  there's no surprise about which authentication backend works
  (whatever your runner's `git` knows about).
- After cloning, parses HEAD to fill in the `commit_sha` and `branch`
  outputs so downstream steps don't have to call `git rev-parse`
  themselves.
- Stderr is captured (not streamed) so a noisy clone can't corrupt the
  component's JSON output contract.

## Output

`commit_sha`, `branch`, `dest`.
