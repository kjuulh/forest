# Destinations

A destination is a physical or logical deployment target within an environment. It's where your code actually runs.

## What Is a Destination?

While an [environment](environments.md) is a logical stage (dev, staging, prod), a destination is a specific place within that stage:

- A Kubernetes cluster
- A Terraform workspace
- An ECS service
- A Flux GitOps repository

Each destination has a **type** that determines which component handles its deployment hooks.

## Creating Destinations

```bash
forest destination create \
  --organisation my-org \
  --name k8s-prod-eu \
  --environment prod \
  --type forest/kubernetes@1

forest destination create \
  --organisation my-org \
  --name infrastructure-prod \
  --environment prod \
  --type forest/terraform@1
```

## Destination Types

Destination types follow the format `{provider}/{name}@{version}`:

```bash
# List available types
forest destination types
```

Common types:

| Type | Description |
|------|-------------|
| `forest/flux@1` | GitOps deployment via Flux v2 |
| `forest/kubernetes@1` | Kubernetes deployment via manifests |
| `forest/terraform@1` | Terraform apply/plan |

## Terraform state

A `forest/terraform@1` destination keeps state in forest-server, behind an
HTTP backend on `FOREST_TERRAFORM_V1_HOST` (internal-only in production). Every
request to it must carry a credential signed for that one state; runs get one
automatically.

| Server variable | |
|---|---|
| `FOREST_TERRAFORM_STATE_SIGNING_KEY` | Base64, at least 32 bytes. Signs state credentials. Its own secret, so rotating `ACCESS_TOKEN_SECRET_KEY` does not revoke state credentials and rotating this does not sign anybody out. **Optional:** unset, forest derives the key from `ACCESS_TOKEN_SECRET_KEY` and logs a warning at startup. Rotating it revokes every outstanding state credential, including a running apply's, so rotate when nothing is applying. |
| `FOREST_TERRAFORM_STALE_LOCK_MINUTES` | Default 15 (flag `--terraform-stale-lock-minutes`). How long a lock with no recorded holder must have been held before a release may clear it; never less than 60 minutes; see [State locks](#state-locks). Forest-server after v0.3.18. |
| `FOREST_REPLICA_GONE_SECS` | Default 1200, minimum 120 (flag `--replica-gone-secs`). Seconds without a heartbeat after which a server replica that did not shut down cleanly (crash, kill, stall) counts as gone, so the locks its ended releases left can be cleared. `FOREST_REPLICA_NAME` (else `HOSTNAME`) names the replica. Forest-server after v0.3.18. |

Generate one with `openssl rand -base64 32`.

### State locks

Terraform locks the state for every plan and apply. Since forest-server after
v0.3.18 the backend keeps what Terraform sends with the lock (ID, operation,
who, version, created) and who took it: an in-process release and the server
replica running it, a remote runner and its release, or the person or app
whose `forest state env --write` credential was used. A run that finds the
state locked shows that holder in Terraform's "Lock Info"
(`Who: forest release <id> on <replica> (<terraform's who>)`), and
`forest state versions` prints it under `LOCKED by <lock id>`:

```text
LOCKED by c7081117-...
  holder: forest release a2fe3766-... on ip-10-0-1-23#5f0c9a1e
  release: a2fe3766-... (failed)
  operation: OperationTypeApply
  who: root@ip-10-0-1-23
  since: 2026-11-19T14:02:11+00:00
```

Older servers record only the lock ID, so `versions` prints just the first
line, and a lock taken before the upgrade has no recorded holder.

Each replica records a heartbeat every 30 seconds. A replica that drains at
shutdown and stops every release it ran marks itself stopped; a replica that
just goes quiet (crash, kill, a stalled database connection) counts as gone
only after `FOREST_REPLICA_GONE_SECS` (default 1200) without a heartbeat,
because a stalled replica may still be applying. Forest clears a lock only
when its holder is known to be dead:

- When an in-process release fails, including one the drain interrupts at
  shutdown, the replica running it releases that release's locks once its
  Terraform process has exited or been killed.
- Before an in-process plan or apply, and before handing a release to a
  remote runner, forest checks the destination's state lock and writes what
  it found to the release log. It releases a lock only when it belongs to an
  in-process release that has ended and whose replica is stopped (at once,
  the usual case after a redeploy) or gone, or when it has no recorded holder,
  is older than 60 minutes (or `FOREST_TERRAFORM_STALE_LOCK_MINUTES` if
  higher) and no other release on the destination is assigned or running.
- Everything else is left and reported: a release still running
  (`locked by release <id> on replica <r>, still running`), an ended release
  whose replica is still alive (`..., still alive`) or only silent
  (`..., not heard from since <time>`), a remote runner's lock,
  and a person's or app's lock (`locked by user:<id> since <time>; clear it with
  forest state unlock`).

The release reaper fails releases that lost their heartbeat or ran too long,
but never removes their locks: Terraform may still be applying.

If a run still reports the state locked, `forest state versions` names the
holder; `forest state unlock` (Admin) clears it by hand once you know it is
not in use.

## Mapping in Configuration

In `forest.cue`, destinations are mapped per environment:

```cue
env: {
    prod: {
        destinations: [
            {destination: "k8s-prod-eu", type: "forest/kubernetes@1"},
            {destination: "k8s-prod-us", type: "forest/kubernetes@1"},
            {destination: "infrastructure-prod", type: "forest/terraform@1"},
        ]
    }
}
```

Destination names support glob patterns in trigger configurations — for example, `infrastructure-prod.*` matches all destinations starting with `infrastructure-prod`.

## Release Targeting

When releasing, you can target by environment (all destinations) or specific destinations:

```bash
# Release to all destinations in prod
forest release release --environment prod

# Release to specific destinations
forest release release --destination k8s-prod-eu --destination k8s-prod-us
```

## Destination State

View what's currently deployed to each destination:

```bash
forest project releases --organisation my-org --project my-service
```

This shows the current release state per destination — what version is deployed, when it was last updated, and the release status.

## CLI Commands

```bash
forest destination create --organisation my-org --name k8s-dev --environment dev --type forest/kubernetes@1
forest destination update --organisation my-org --name k8s-dev
forest destination delete --organisation my-org --name k8s-dev
forest destination list --organisation my-org
forest destination types
```

## Drift (`forest/terraform@1`)

Terraform state records what forest last applied. It doesn't tell you whether someone has changed a resource in a console since. To check:

```bash
forest release drift --environment dev              # every destination this project deployed to
forest release drift -e prod -d prod/eu-west-1/core # one destination
```

For each destination the server takes the artifact of that destination's last successful deploy and runs `terraform plan -refresh-only -detailed-exitcode -lock=false` against it. It changes nothing: no state write, no lock, so it can't block or be blocked by a release.

The exit status is `0` only when at least one destination was checked and every
result is in sync. It is `2` when drift was found and no check failed (the
refresh-only plan is printed unless `--quiet` is set). It is `1` for an empty,
unsupported, unknown or failed result, or an RPC failure. An incomplete check
takes precedence over drift elsewhere; unsupported destination types are not a
clean result.

That is the behaviour after v0.3.18. Through v0.3.18 an unsupported destination
type counts as in sync and drift outranks a failed check, so `0` does not mean
every destination was checked. The exit-status line of `forest release drift
--help` says which one a CLI has.

In CI, check that `forest release --help` lists `drift` before relying on it:
CLIs before v0.3.14 don't have the command.

The request is `forest.v1.ReleaseService/CheckDrift` with `project`
(`organisation` and `project`), `environment`, and optional `destination`.
The response contains per-destination status, artifact ID, output and error.
It does not annotate, apply or release anything. An environment without a
successful deploy has no comparison baseline and fails.

The check runs synchronously in the request. Scheduling it and showing the result in forage are follow-ups.

