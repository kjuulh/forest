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

### Release pipeline diagram

In the fork's Forage UI, an individual release's **Release pipeline** workspace
sits below a compact context header. Default project **Releases** and the
all-project overview remain timeline-first, with no large graph or graph fetch.
Open the project's **Graph** tab beside Releases for its optional **Environment
status** workspace. Environment lanes wrap after four destination cards; shared
connectors keep the lanes separate. Each physical destination has one card with
current and incoming context. On this tab, expanding an active entry lazily
loads its full pipeline. Original timelines and stage disclosures remain;
there is no lower-resolution replacement.

The server lays out dependency columns, with separate nodes for each stage's
destination runs, plan approvals and current policy evaluations. Plan and
deploy executions remain separate; a newer release's destination status does
not rewrite an older execution. Text labels distinguish running, next, queued,
waiting, blocked, failed and terminal states.

Within a pipeline, completed, active and future nodes all remain visible.

The pipeline camera automatically focuses current work; **Fit** shows the entire
graph. Desktop environment workspaces start with **Fit**; **Active** frames an
active environment and its destinations. Drag or middle-drag to pan, scroll to
zoom, or expand to fullscreen. Cards include environment, execution progress,
duration and waiting/failure context. Select a node to pin its inspector.
Pipeline details also show available plan previews.

An expanded Graph-tab pipeline fetches output only when its inspector's **Plan**
tab opens; the environment workspace has no plan/log controls. Collapsed and initially
inactive entries make no pipeline graph request.
Detail approval controls submit the existing CSRF-protected forms; overview
controls link to the release. Keyboard users can Tab to nodes, press Enter to
select, and use F/A for Fit/Active.
Details poll every ten seconds while unfinished, visible and on-screen. Overview
refreshes coalesce at most once per ten seconds while visible; only expanded
visible pipelines fetch their graph. Both preserve manual views and pinned
inspection. Running emphasis respects reduced motion.

Each Graph-tab destination card keeps current completed operations alongside
incoming changes, with a pending count when several are queued. Its inspector
retains each operation's provenance, state, reason and timing. A newer completion
does not hide queued work, and pinned selection follows the physical target
across operation changes. Plan completion is not deployment success; missing
operation provenance is labelled. Configured destinations without current state
are unknown, not deployed.

The shared renderer and real Plan output were browser-verified on dev and
production at revision `a2e01b20`. The subsequent opt-in navigation and compact
header revision has separate checks recorded in Forage spec 018.

## Combined Command

`forest release create` prepares and annotates, auto-detecting organisation, project and git context:

The no-target form below is in **v0.3.17** and newer. Released v0.3.16 requires an environment and skips triggers; use `prepare` + `annotate --source-type ci` for a version-independent trigger flow. A build from source may print an older number, so check `create --help`.

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

- **Assigned** or **running** releases whose runner has sent no heartbeat for 90 seconds fail with "release heartbeat lost". Up to forest-server v0.3.18, a server restart mid-deploy ended in-process releases this way; later servers drain first (see [Server restarts](#server-restarts)).
- **Assigned** releases that haven't started within 5 minutes are timed out
- **Running** releases that haven't completed within 1 hour are timed out

Every release that starts sends a `RELEASE_STARTED` notification, whether `forest release` or a matching trigger began it (triggered releases since forest-server after v0.3.18; earlier servers sent it for manual releases only).

This prevents releases from being permanently stuck if a runner crashes. A reaped release sends a `RELEASE_FAILED` notification like any other failure, so Slack messages and webhooks show it as failed (since forest-server after v0.3.18; earlier servers sent none).

## Server restarts

Since forest-server after v0.3.18, a server that receives SIGTERM (a deploy or scale-in) drains instead of dropping the releases it is running:

1. It stops claiming queued releases at once, so another replica picks them up, and its health check reports `accepting-releases` as unhealthy.
2. Releases already running inside the server get `FOREST_DRAIN_TIMEOUT_SECS` (default 90 seconds, flag `--drain-timeout-secs`) to finish and are recorded as usual.
3. A release still running after that is interrupted: Terraform gets SIGINT and up to 15 seconds to stop cleanly, save its state and release its state lock.
4. Whatever is left then fails immediately with "interrupted by forest-server shutdown" and a `RELEASE_FAILED` notification, rather than 90 seconds later as "release heartbeat lost". The next queued release for that destination starts on another replica.

Interrupted releases are failed, not re-queued: a half-applied Terraform run, a saved plan that no longer matches state, or a provider call already in flight is not safe to start again without someone looking at it. Re-run the release once you have checked the destination. If Terraform had to be killed rather than stopping on SIGINT, its state lock stayed held on forest-server v0.3.18 and earlier and needed `forest state unlock`; after v0.3.18 the replica releases the lock itself once Terraform has exited or been killed, and a lock left by a replica that disappeared is cleared by the next release once that replica has marked itself stopped or been silent for `FOREST_REPLICA_GONE_SECS` (default 20 minutes) (see [State locks](destinations.md#state-locks)). The reaper never removes locks.

The server exits within the drain timeout plus 25 seconds, so the orchestrator's stop timeout must be at least that (120 seconds on ECS with the default). Remote runners are not failed when the server they are connected to shuts down: the server closes their stream after the drain and they reconnect to another replica. A runner reconnects only after the release it is running ends, so a remote release that outlives the drain by more than the 90-second heartbeat window is still reaped.
