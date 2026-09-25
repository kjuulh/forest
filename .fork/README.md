# How this fork stays in sync with upstream

This repository (`kjuulh/forest`, branch `kjuulh/gitea-fork`) is
[`understory-io/forest`](https://github.com/understory-io/forest) plus an
overlay: Woodpecker CI, deployment, the Rawpotion distribution and branding.
Code moves **one way**, from upstream into the fork.

## The rule

Product changes are made upstream first, as a PR on `understory-io/forest`.
They reach the fork through the daily sync. They are not committed here
first and then copied upstream.

Before this, fixes were written in the fork, cherry-picked into upstream PRs,
squash-merged there, and merged back. Every change landed twice, and every
sync was a hand-resolved merge (nine conflicting files on 2026-09-24).

## What the fork may change

Every file that differs from the upstream commit last merged
(`.fork/upstream-base`) is declared in one of these files:

| File | Holds | On sync |
|---|---|---|
| `overlay` | paths the fork owns outright (`.woodpecker/`, `deployment/`, …) | cannot conflict: upstream never has them |
| `removed` | upstream files the fork deletes | deleted again after every merge |
| `prefer-ours` | files where the fork owns specific lines (the CLI version, the private `canopy-otel` dependency removed) | conflicting hunks keep the fork's side; upstream's other changes land |
| `carries` | every other change to upstream code, grouped by reason and exit plan | stops the sync if upstream edits the same lines, unless the group is `[take-upstream #N]` and PR #N has merged |

`scripts/fork-check.sh` enforces this, and `.woodpecker/fork-check.yaml` runs
it on every push and pull request. It fails on an undeclared change and warns
about carries that no longer differ from upstream.

## The daily sync

`.woodpecker/upstream-sync.yaml` runs on the `upstream-sync` cron:

1. fetches upstream `main` with a read-only deploy key;
2. runs `scripts/fork-sync.sh`, which merges it into a new branch
   `sync/upstream-<date>`, applies the resolutions above, re-resolves the
   `Cargo.lock` files with `cargo metadata`, drops stale carries, and commits
   in the usual "merge: reconcile Gitea fork with Understory main" form;
3. runs the forest and forage test suites the way upstream's CI does
   (Postgres, NATS, MinIO, cue, OpenTofu);
4. pushes the branch and fast-forwards `understory/main`.

It never pushes `kjuulh/gitea-fork`, because that branch builds and ships
images. **Merging the sync branch is the one manual step**:

```bash
git fetch origin
git switch kjuulh/gitea-fork && git merge --ff-only origin/sync/upstream-<date>
git push origin kjuulh/gitea-fork
```

The same by hand, from any checkout that has upstream as a remote:

```bash
git fetch upstream main
scripts/fork-sync.sh --upstream upstream/main --base origin/kjuulh/gitea-fork
```

## When it goes red

- **"declared carry: upstream changed the same lines"**: upstream edited
  lines the fork carries. Resolve by hand (`scripts/fork-sync.sh --help`,
  "Resolving by hand"). Where you can, move the fork's text into a
  fork-owned file (an overlay path) so the conflict does not recur. The
  branding docs are the likeliest source.
- **"not declared in .fork/"** or a fork-check failure: someone committed
  product code in the fork. Open it as an upstream PR and list it in
  `carries` under that PR until it lands.
- **Tests fail**: upstream plus the fork does not pass. Nothing was pushed.
  Reproduce with `fork-sync.sh` locally.

## Making a change

- **A fix or feature in forest or forage:** open a PR on
  `understory-io/forest`. If the fork needs it before upstream merges it,
  commit the same change here and add its files to `carries` under
  `## [take-upstream #<PR>] …`. When the PR lands, reworked or not, the sync
  takes upstream's version and drops the carry.
- **CI, deployment, distribution:** use an overlay path. If a new path is
  fork-owned, add it to `overlay`.
- **Branding in shared files:** list it in `carries` under the branding group.
  It stays until branding is configuration upstream.

## Credentials

- `upstream_deploy_key` (Woodpecker secret, cron only): a read-only deploy
  key on `github.com/understory-io/forest`, titled `kjuulh/forest upstream-sync
  (read-only)`. It can read that one repository.
- `gitea_sync_key` (Woodpecker secret, cron only): a deploy key with write
  access on `git.kjuulh.io/kjuulh/forest`, titled `upstream-sync`. The
  pipeline pushes only `sync/*` and `understory/main` with it, but Gitea does
  not restrict deploy keys by branch.

Rotate either by deleting the key in the forge's repository settings, adding
a new one, and updating the secret.
