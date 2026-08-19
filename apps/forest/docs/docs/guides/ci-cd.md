# CI/CD Integration

Forest is designed to be driven from CI/CD pipelines. This guide covers common integration patterns.

## Authentication

Generate a token for CI/CD:

```bash
# One-time setup. Writes the raw token to stdout and everything else to
# stderr, so it pipes straight into a secret store without leaking:
forest auth token create --name "ci-bot" | gh secret set FOREST_TOKEN --repo <org>/<repo>
```

:::note
This is a *personal* access token — it carries the permissions of whoever
created it. There is no machine/service-account token today; `forest
organisation` has no `app` subcommand despite what earlier revisions of this
guide claimed. Prefer a token created by an account that only holds the access
CI actually needs.
:::

Set the token in your pipeline:

```bash
export FOREST_TOKEN="<your-app-token>"
export FOREST_SERVER="https://forest.example.com:4040"
```

`FOREST_TOKEN` is read directly by the gRPC auth interceptor and bypasses the
interactive login entirely — no browser, no refresh, no local state file. It is
the only credential a CI publish needs.

Forest never prompts for credentials during a publish. An unattended run with
no token fails immediately, before any upload, with a message naming what is
missing rather than a transport error from somewhere inside the interceptor.

## Publishing a component on a tagged release

Publishing a component from CI is a reusable workflow rather than something
each repo assembles itself:

```yaml
# .github/workflows/release.yml in the component's repo
name: Release
on:
  push:
    tags: ["v*"]

jobs:
  publish:
    uses: understory-io/forest/.github/workflows/forest-publish.yml@main
    with:
      tag: ${{ github.ref_name }}
      context-name: understory-prod
      server: https://api.forest.understory.sh
    secrets:
      forest-token: ${{ secrets.FOREST_TOKEN }}
      # Org-wide secret, already visible to every private repo.
      forest-repo-token: ${{ secrets.GO_PRIVATE_MODULES_PAT }}
```

### The version comes from `forest.cue`

`forest.component.version` is the source of truth. The workflow does not set
it — it *checks* it, and fails if the tag implies a version the manifest does
not declare.

Keeping the two in step is release-please's job. Annotate the version line so
its `generic` updater rewrites it:

```cue
forest: component: {
    name:    project.name
    version: "0.1.7" // x-release-please-version
}
```

```json
// release-please-config.json
{
  "packages": {
    ".": {
      "release-type": "simple",
      "package-name": "mytool",
      "include-component-in-tag": false,
      "extra-files": [{ "type": "generic", "path": "forest.cue" }]
    }
  }
}
```

Merging the release PR bumps the manifest and pushes the tag in one commit, so
the tag and the published version cannot drift apart. The updater only touches
the annotated line — a dependency pinned elsewhere in the file is left alone.

### Prereleases override the manifest

A semver prerelease tag (`v0.2.0-rc.1`, `v0.1.7-ci.1`) is the one case where
the tag wins. This exists so the pipeline can be exercised, or a release
candidate cut, without a commit bumping the manifest. The workflow passes the
version through `FOREST_COMPONENT_VERSION`, which both `forest run build` and
`forest publish` read — so the version stamped into the binary matches the one
the registry records.

Doing this by hand:

```bash
export FOREST_COMPONENT_VERSION=0.2.0-rc.1
forest run build && forest publish
```

Precedence is `--version` > `FOREST_COMPONENT_VERSION` > `forest.cue`. Prefer
the environment form: `--version` reaches the upload but not the build, leaving
the binary reporting the manifest's version. `forest publish` warns when it
detects this.

:::warning
Forest does not exclude prereleases when resolving a bare `<org>/<name>` — it
takes the semver maximum. A prerelease numbered above the current release
becomes what everyone installs. Number test prereleases *below* the current
release (`0.1.7-ci.1`, not `0.1.99-ci.1`), or publish them to a dev registry.
:::

### The build runs in CI, not on the registry

The registry hosts binaries; it does not build them. `forest run build`
dispatches the depended-on build component, which writes
`.forest/component/output/<os>/<arch>/<name>`, and `forest publish` uploads
whatever it finds there. So the runner needs the component's toolchain, and the
build must happen before the publish. The reusable workflow does both.

By default one Linux runner cross-compiles the entire declared matrix, which is
right for anything that cross-compiles cleanly (pure Go with CGO off, most
Rust). When a platform has to be built natively — a cgo dependency on a system
framework, an awkward native crate — split the build across runners and let each
leg build its own share:

```yaml
    with:
      build-matrix: >-
        [{"name":"linux","runner":"ubuntu-latest","targets":"linux/amd64,linux/arm64"},
         {"name":"macos","runner":"macos-latest","targets":"macos/arm64"}]
```

Each leg sets `FOREST_BUILD_TARGETS` to its `targets`, uploads its artifacts,
and the single publish job reassembles them. A selector naming a platform the
component does not declare is an error, so a typo fails the leg instead of
quietly leaving a hole in the published platform set.

The publish runner must itself be one of the built platforms: `forest publish`
derives the manifest by executing the freshly built binary for its own platform
and asking it to describe itself.

### Re-running a tag

Safe. The registry refuses a version that is already published, so a re-run
fails cleanly rather than double-publishing or overwriting. A half-finished
upload is rolled back by the CLI's abort-on-drop guard, which frees the version
to be retried.

### Adding this to a new repo

1. Annotate `forest.cue` and add `release-please-config.json` +
   `.release-please-manifest.json` as above, seeding the manifest with the
   version already published.
2. Copy `release-please.yml` and `release.yml` from
   [pgjump](https://github.com/understory-io/pgjump/tree/main/.github/workflows).
3. Add the secrets below.
4. Ensure the repo's toolchain is pinned in `mise.toml`, including `cue` —
   forest shells out to it and does not vendor it.

| Secret | Why |
|---|---|
| `FOREST_TOKEN` | Forest token with write access to the org. `forest auth token create --name <repo>-ci`. **The only one you have to create.** |
| `GO_PRIVATE_MODULES_PAT` | Org-wide, visible to every private repo — no setup. Pass it as `forest-repo-token`: `install.sh` fetches the CLI with `gh release download` from the **private** forest repo, and the caller's automatic `GITHUB_TOKEN` is scoped to the calling repository only. The same secret doubles as the private-Go-module PAT for components that need one. |
| `RELEASE_PR_GITHUB_ACTIONS_WORKFLOW` | Org-wide, no setup. Used by release-please. It has to be a PAT (or app token) rather than `GITHUB_TOKEN` — a tag pushed with `GITHUB_TOKEN` does not trigger other workflows, so the release would tag but never publish. |

`understory-io/forest` must also allow its workflows to be used by other
repositories in the org (Settings → Actions → Access).

## Basic Pipeline

### GitHub Actions

```yaml
name: Deploy
on:
  push:
    branches: [main]

jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Forest
        run: cargo install forest-cli

      - name: Annotate Release
        env:
          FOREST_TOKEN: ${{ secrets.FOREST_TOKEN }}
          FOREST_SERVER: ${{ secrets.FOREST_SERVER }}
        run: |
          forest release annotate \
            --organisation my-org \
            --project-name my-service \
            --context-title "${{ github.event.head_commit.message }}" \
            --commit-sha "${{ github.sha }}" \
            --commit-branch "${{ github.ref_name }}" \
            --source-type ci \
            --run-url "${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }}"
```

## Trigger-Based Flow

The recommended pattern is to use [triggers](../concepts/triggers.md) instead of explicit release commands in CI. Your CI pipeline only annotates — triggers handle the rest:

```yaml
# CI only annotates
- name: Annotate
  run: |
    forest release annotate \
      --organisation my-org \
      --project-name my-service \
      --context-title "$(git log -1 --format=%s)" \
      --commit-sha "$(git rev-parse HEAD)" \
      --commit-branch "$(git branch --show-current)" \
      --source-type ci
```

Configure triggers on the server side:

```bash
# Auto-deploy to staging on main
forest project trigger create ci-staging \
  --branch "^main$" \
  --source-type "^ci$" \
  --target-environment staging

# Auto-deploy to prod via pipeline on tags
forest project trigger create ci-prod \
  --branch "^v[0-9]" \
  --target-environment prod \
  --use-pipeline
```

This separates **what** gets deployed (CI annotation) from **where** and **how** (server-side triggers and policies).

## Explicit Release Flow

For full control, annotate and release explicitly:

```yaml
- name: Release to staging
  run: |
    forest release release \
      --organisation my-org \
      --project my-service \
      --environment staging

- name: Wait for staging
  run: |
    forest release wait "$INTENT_ID"
```

## Policy Evaluation

Before releasing, check if policies allow it:

```yaml
- name: Check policies
  run: |
    forest project policy evaluate \
      --organisation my-org \
      --project my-service \
      --environment prod
```

## Release Context

The annotation captures rich metadata about the CI context:

| Flag | Description | Example |
|------|-------------|---------|
| `--source-type` | Where the release came from | `ci`, `manual`, `webhook` |
| `--source-username` | Who triggered it | `ci-bot` |
| `--source-email` | Email of the triggerer | `ci@example.com` |
| `--run-url` | Link back to the CI run | GitHub Actions URL |
| `--context-title` | Human-readable title | Commit message |
| `--context-description` | Longer description | PR body |
| `--context-web` | Link to the change | Commit URL |
| `--context-pr` | Pull request link | PR URL |
| `--commit-sha` | Exact commit | `abc123def` |
| `--commit-branch` | Source branch | `main` |
| `--commit-message` | Full commit message | `Add feature X` |

## Image Tagging with `--set`

The `--set` flag on `forest release create` lets CI override config values without modifying `forest.cue`. This is the recommended way to pin image tags from CI:

```bash
# In your CI pipeline (e.g. Dagger, GitHub Actions, Drone):
IMAGE_TAG=$(git rev-parse --short HEAD)

# Build and push the image
docker build -t git.example.io/org/my-service:$IMAGE_TAG .
docker push git.example.io/org/my-service:$IMAGE_TAG

# Release with the exact tag that was just built
forest release create --env dev \
  --set my-org/service.tag=$IMAGE_TAG
```

Your `forest.cue` keeps `tag: "latest"` as a sensible default for local development, while CI always pins to the exact build:

```cue
config: {
    name:  "my-service"
    image: "git.example.io/org/my-service"
    tag:   "latest"  // overridden by CI via --set
}
```

Multiple overrides can be combined:

```bash
forest release create --env prod \
  --set my-org/service.tag=$IMAGE_TAG \
  --set my-org/service.env_vars.BUILD_SHA=$COMMIT_SHA
```

All `--set` overrides are recorded in the release annotation metadata for audit purposes.

## Event Streaming

Subscribe to release events for notifications or dashboards:

```bash
forest notifications subscribe \
  --organisation my-org \
  --project my-service \
  --resource-types release \
  --actions status_changed
```
