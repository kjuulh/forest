# Releases

Releases are the core operation in Forest. They represent the act of deploying an artifact to one or more destinations, tracked as an event-sourced lifecycle.

## Release Lifecycle

Every release goes through a well-defined state machine:

```
Queued → Assigned → Running → Succeeded
                             → Failed
                             → TimedOut
                             → Cancelled
                             → Superseded
```

| State | Meaning |
|-------|---------|
| **Queued** | Waiting for a runner to pick it up |
| **Assigned** | A runner has claimed the work |
| **Running** | Deployment is in progress |
| **Succeeded** | Deployment completed successfully |
| **Failed** | Deployment failed |
| **TimedOut** | No progress within timeout (5 min assigned, 1 hr running) |
| **Cancelled** | Manually or automatically cancelled |
| **Superseded** | Replaced by a newer queued release under a `supersede_pending` policy |

A release can also stay **Queued** while a [soak-time policy](policies.md) for its environment is not yet met.

Forest enforces a **partial unique index**: only one release can be in-flight per project+destination at a time.

## The Three Steps

### 1. Annotate

An annotation creates an immutable record of what you're deploying and why:

```bash
forest release annotate \
  --annotation-only \
  --organisation my-org \
  --project-name my-service \
  --context-title "Deploy v1.2.3" \
  --commit-sha abc123 \
  --commit-branch main
```

The annotation captures three categories of metadata:

- **Source**: username, email, source type (manual, CI, webhook), run URL
- **Context**: title, description, web link, PR link
- **Reference**: commit SHA, branch, message, version, repository URL

!!! note
    This explicit-release recipe skips trigger evaluation with `--annotation-only`. For the normal pipeline-first flow, prepare then annotate without that flag, inspect the configured triggers and whole pipeline first, and do not also make the separate release call below.

### 2. Release

Execute the deployment:

```bash
forest release release <slug> \
  --organisation my-org \
  --project my-service \
  --environment prod
```

This creates one **release intent** with a release per destination. Each destination's release is an independent unit of work that a runner picks up and executes.

The slug printed by `annotate` names the artifact. `forest release release <slug> -e <env>` releases that same artifact again: this is how an artifact is promoted to another environment, or rolled back by releasing an earlier slug. There is no separate rollback command.

### 3. Follow

Stream progress in real-time:

```bash
forest release show <slug> --follow
```

The wait stream includes:

- Log lines from the runner
- Status transitions
- Pipeline stage updates (if using a pipeline)

## Combined Command

`forest release create` prepares and annotates, auto-detecting organisation, project and git context:

The no-target form below is in unreleased source after **v0.3.16**. Released v0.3.16 requires an environment and skips triggers; use `prepare` + `annotate --source-type ci` for a version-independent trigger flow. A local build may still print `0.3.16`, so check `create --help`.

```bash
forest release create --source-type ci       # triggers decide; usually the project's pipeline
forest release create --environment dev      # release to dev directly, triggers skipped
```

Without `--environment` or `--destination`, matching triggers release the annotation, and they may deploy through the whole pipeline, including prod. With a target, it releases there directly **unless `--skip-commit` or `--pipeline` is supplied**. `--skip-commit` enables triggers even with `-e dev`; `--pipeline` runs the whole pipeline, not just dev. Neither is an environment fence.

## Force Release

If a release is already queued for a destination, use `--force` to cancel it and jump to the front. `--force` does not bypass [policies](policies.md):

```bash
forest release release --environment prod --force
```

## Release with Pipeline

Use `--pipeline` to route the release through the project's configured [pipeline](pipelines.md). The whole pipeline runs, whatever `--environment` says:

```bash
forest release release <slug> --environment dev --pipeline
```

## Event Sourcing

Releases are stored as an append-only event log (`release_events` table) with a materialized projection (`release_states` table). This provides:

- Full audit trail of every state transition
- Consistent state even under concurrent access
- NATS-based real-time notifications

## Release Reaper

A background process monitors stuck releases:

- **Assigned** releases that haven't started within 5 minutes are timed out
- **Running** releases that haven't completed within 1 hour are timed out

This prevents releases from being permanently stuck if a runner crashes.
