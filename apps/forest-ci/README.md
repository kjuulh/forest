# forest-ci

A single OCI image used as a **Woodpecker plugin** today and a
**GitHub Action** later. Wraps `forest release {create,prepare,annotate}`
so every personal-app repo can drop ~30 lines of curl bootstrap from
its CI.

The plugin bakes pinned versions of forest, cue, and deno — consumers
only need to declare the action and its inputs.

## Image

`git.kjuulh.io/kjuulh/forest-ci:<tag>` — built by
`.woodpecker/forest-ci-image.yaml` on every push that touches
`apps/forest-ci/*`. Multi-arch (amd64 + arm64).

Tags:
- `:rawpotion` — rolling head of the fork branch
- `:rawpotion-<sha>` — immutable per-commit pin

## Usage (Woodpecker)

```yaml
steps:
  release:
    image: git.kjuulh.io/kjuulh/forest-ci:rawpotion
    settings:
      action: release-create
      forest_server: https://forest.i.kjuulh.io
      forest_token:
        from_secret: forest_token
      environment: dev
      # Shorthand: --set kjuulh/service.tag=<value>. Most personal apps
      # only need this single set; use `extra_sets` for the rest.
      image_tag: rawpotion-${CI_COMMIT_SHA}
```

For repos with a single project (controllers etc.) where `forest.cue`
lives at the root rather than under `deployment/projects/*/`, point
`projects_dir` at it:

```yaml
    settings:
      action: release-create
      projects_dir: .         # or "deployment", whichever holds forest.cue
      ...
```

## Inputs

All inputs read three env-var prefixes in this precedence:
`FOREST_*` (explicit override) → `PLUGIN_*` (Woodpecker) → `INPUT_*`
(GitHub Actions). Hosts that follow either convention work without
any code change.

| Input            | Required          | Description |
|------------------|-------------------|-------------|
| `action`         | yes               | `release-create` / `release-prepare` / `release-annotate` |
| `forest_server`  | yes               | URL of the forest server (e.g. `https://forest.i.kjuulh.io`) |
| `forest_token`   | yes (secret)      | Token with release+annotate scope |
| `environment`    | yes for `release-create` | Forest env name (`dev`, `prod`, …) |
| `projects_dir`   | no (default `deployment/projects`) | Where to look for projects. If this dir itself contains `forest.cue`, treat it as a single project; otherwise iterate subdirs that contain one |
| `image_tag`      | no                | Shorthand: appends `--set kjuulh/service.tag=<value>`. **Only works for projects whose primary deployed component is `kjuulh/service`.** For other components (e.g. `rawpotion/controller-service`) use `extra_sets` with the right key — otherwise the `--set` lands on a non-existent path and the deployed image stays at whatever `forest.cue` declares (typically `:latest`, so k8s won't roll). |
| `extra_sets`     | no                | Newline-separated `key=value` list; one `--set` per line. Use this for any component path other than `kjuulh/service.tag`, e.g. `rawpotion/controller-service.tag=main-<sha>`. |
| `cue_registry`   | no                | Auto-derived from `forest_server` (`forest.sh=registry.<host>,registry.cuelang.org`) |
| `rust_log`       | no (default `forest=info,component=info`) | Forwarded to the forest CLI; `component=info` surfaces deno hook stderr |

## CI metadata

Commit SHA, branch, message, repo URL, and run URL are auto-detected:

- **Woodpecker**: `CI_COMMIT_SHA`, `CI_COMMIT_BRANCH`, `CI_COMMIT_MESSAGE`, `CI_REPO_URL`, `CI_PIPELINE_URL`
- **GitHub Actions**: `GITHUB_SHA`, `GITHUB_REF_NAME`, `GITHUB_EVENT_HEAD_COMMIT_MESSAGE`, `GITHUB_REPOSITORY`, `GITHUB_RUN_ID`

Override any with explicit `CI_SHA`, `CI_BRANCH`, `CI_MSG`, `CI_REPO`,
`CI_RUN`, `CI_SOURCE_TYPE` env vars on the step.

## Future: GitHub Action

When we add `action.yml` to this directory, the same image runs as a
Docker GitHub Action. No entrypoint changes — inputs flow in via
`INPUT_*` env vars, which the script already reads.
