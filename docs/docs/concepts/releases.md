# Releases

Releases are the core operation in Forest. They represent the act of deploying an artifact to one or more destinations, tracked as an event-sourced lifecycle.

## Release Lifecycle

Every release goes through a well-defined state machine:

```
Queued → Assigned → Running → Succeeded
                             → Failed
                             → TimedOut
                             → Cancelled
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

Forest enforces a **partial unique index**: only one release can be in-flight per project+destination at a time.

## The Three Steps

### 1. Annotate

An annotation creates an immutable record of what you're deploying and why:

```bash
forest release annotate \
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
    Annotations can trigger automatic releases if [triggers](triggers.md) are configured. Use `--annotation-only` to skip trigger evaluation.

### 2. Release

Execute the deployment:

```bash
forest release release \
  --organisation my-org \
  --project my-service \
  --environment prod
```

This creates one **release intent** per destination. Each intent is an independent unit of work that a runner picks up and executes.

### 3. Wait

Stream progress in real-time:

```bash
forest release wait <release-intent-id>
```

The wait stream includes:

- Log lines from the runner
- Status transitions
- Pipeline stage updates (if using a pipeline)

## Combined Command

`forest release create` bundles all three steps:

```bash
forest release create --environment dev
```

It auto-detects organisation, project, and git context from the local environment.

## Force Release

If a release is already queued for a destination, use `--force` to cancel it and jump to the front:

```bash
forest release release --environment prod --force
```

## Release with Pipeline

Use `--pipeline` to route the release through the project's configured [pipeline](pipelines.md):

```bash
forest release release --environment prod --pipeline
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
