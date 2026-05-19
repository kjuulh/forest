# Your First Release

Releases in Forest follow a three-step lifecycle: **prepare**, **annotate**, and **release**. The `forest release create` command bundles all three into a single operation.

## The Quick Way

From your project directory:

```bash
forest release create --environment dev
```

This automatically:

1. **Prepares** deployment manifests from your component templates
2. **Annotates** the release with git context (commit SHA, branch, message)
3. **Releases** to all destinations in the target environment
4. **Streams** progress until the release completes or fails

## Step by Step

For more control, run each step separately.

### 1. Prepare

Generate deployment manifests by invoking component hooks:

```bash
forest release prepare
```

This reads your `forest.cue`, renders component templates, and invokes `prepare` hooks. The output is a set of manifests ready for deployment.

### 2. Annotate

Create a release annotation — an immutable record of what you're deploying and why:

```bash
forest release annotate \
  --organisation my-org \
  --project-name my-service \
  --context-title "Deploy v1.2.3" \
  --context-description "Add user profile feature" \
  --commit-sha "$(git rev-parse HEAD)" \
  --commit-branch "$(git branch --show-current)"
```

The annotation captures:

- **Source**: who triggered the release and from where (CI, manual, etc.)
- **Context**: human-readable title, description, links to PR/web
- **Reference**: commit SHA, branch, version, commit message

Annotations can also trigger automatic releases if [triggers](../concepts/triggers.md) are configured.

### 3. Release

Execute the deployment:

```bash
forest release release \
  --organisation my-org \
  --project my-service \
  --environment dev
```

Optional flags:

| Flag | Description |
|------|-------------|
| `--destination <name>` | Target specific destinations (can be repeated) |
| `--force` | Cancel queued releases and jump to front |
| `--pipeline` | Use the project's release pipeline instead of deploying directly |
| `--no-wait` | Don't stream progress, return immediately |

### 4. Watch

Stream release progress:

```bash
forest release wait <release-intent-id>
```

This shows real-time logs, status transitions, and pipeline stage progress.

## Viewing Release State

See what's deployed where:

```bash
# Per-project release state
forest project releases \
  --organisation my-org \
  --project my-service
```

## Release Lifecycle

Each release goes through these states:

```
Queued → Assigned → Running → Succeeded
                            → Failed
                            → TimedOut
                            → Cancelled
```

- **Queued**: Waiting for a runner to pick it up
- **Assigned**: A runner has claimed the work
- **Running**: Deployment is in progress
- **Succeeded/Failed/TimedOut/Cancelled**: Terminal states

Forest enforces that only one release can be in-flight per project+destination at a time. If you release again while one is queued, use `--force` to cancel the existing one.

---

**Next:** Learn about the [core concepts](../concepts/index.md) that make Forest work.
