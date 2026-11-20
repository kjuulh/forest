# Destinations and deployment providers

A destination is the configured place where an application release goes. It
binds an environment to a versioned destination type and its credentials.

Examples:

- a Forest Runtime namespace and region;
- a customer's Kubernetes cluster;
- a Flux GitOps repository;
- a Terraform workspace;
- an external deployment-provider service.

Application teams use the same `forest release` workflow for every destination.
The destination type owns target-specific planning, apply, and status behaviour;
rollback is available only where the provider/runtime implements it.

## Two deployment modes

### Forest Runtime

`forage/containers@1` is the current managed-runtime implementation. It sends a
container-service resource to a configured Forage cluster and follows the
rollout until success or failure.

This coordinate exposes implementation history; the product name is **Forest
Runtime**. It is a development preview. It currently creates a basic container
service from release and destination metadata and does not yet carry the
isolation, networking, secrets, observability, scaling, durability, or SLO
guarantees required for production hosting.

### Customer infrastructure

Customer-owned infrastructure stays behind a destination provider. Forest
retains the release record, pipeline, policy, approval, scheduling, and status
model while the provider performs the target-specific work.

Current destination types include:

| Type | Execution target |
|---|---|
| `forest/flux@1` | Customer GitOps repository and Flux-managed cluster |
| `forest/kubernetes@1` | Customer Kubernetes cluster |
| `forest/terraform@1` | Customer Terraform/OpenTofu workspace |
| `forest/generic@1` | External service implementing the Forest provider protocol |
| `forage/containers@1` | Managed Forest Runtime preview |

The list reports code that exists, not a production support matrix. Operators
must explicitly approve and configure the types they expose.

## External provider protocol

`forest/generic@1` connects to a service implementing
`forest.provider.v1.DestinationProvider`. The provider owns its metadata schema
and deployment implementation, so adding a target does not require compiling it
into Forest.

The current call direction is control plane → provider. The provider must be
reachable from `forest-server`. `FOREST_GENERIC_PROVIDER_ALLOWED_HOSTS` is
deny-by-default and must allow the endpoint before Forest will send it a
release-scoped token. A provider that can only dial outward requires the
authenticated runner model rather than the current generic-provider path.

Example:

```bash
forest destination create \
  --organisation acme \
  --name prod-ecs \
  --environment prod \
  --type forest/generic@1 \
  --metadata provider_url=https://forest-ecs-provider.internal:4060 \
  --metadata provider_token="$PROVIDER_TOKEN"
```

For generic providers, destination metadata owns settings such as accounts,
clusters, and credentials. Project configuration adds only keys the destination
does not already define; it cannot replace destination values or send
`provider_url`/`provider_token` as provider metadata. Fields the provider declares
sensitive at destination creation cannot come from project configuration even
when unset on the destination. Keep credentials in destination-sensitive metadata,
never in project artifacts.

Fields marked `sensitive` by `Describe` are withheld from destination listing,
even if the creator did not name them in `sensitive_keys`; provider failure
withholds all metadata rather than exposing a credential. Only an organisation
admin or infrastructure service account may reveal a withheld credential or
approve/reject a plan. Ordinary members and org-scoped app tokens cannot
perform those operator actions.

The provider receives the annotated artifact slug, source reference, release
intent ID and artifact ID alongside the execution ID. PLAN must return
nonempty, redacted `plan_output`. Forest persists that output once and passes
its exact bytes as `ExecuteRequest.approved_plan_output` only to a DEPLOY
descending from an approved plan for the same intent, destination and artifact.
A gated DEPLOY without that seal fails; an ungated deploy sends an empty field.

Provider authentication, mTLS, identity, release-token audience binding,
revocation, callback authorization, and cross-tenant tests remain launch gates.
Do not expose an arbitrary provider URL allowlist to untrusted organisation
members.

## Creating a Forest Runtime destination

An operator with a configured runtime endpoint can create:

```bash
forest destination create \
  --organisation acme \
  --name forest-dev \
  --environment dev \
  --type forage/containers@1 \
  --metadata forage_url=https://runtime.internal.example:4050 \
  --metadata namespace=acme \
  --metadata region=eu-west-1
```

This is operator setup, not a claim that the example host exists or that the
runtime is ready for production traffic.

## Creating a customer-infrastructure destination

For a built-in type:

```bash
forest destination create \
  --organisation acme \
  --name k8s-prod-eu \
  --environment prod \
  --type forest/kubernetes@1
```

Use `forest destination types` to inspect the configured server's actual types
and required metadata:

```bash
forest destination types
```

Secrets belong in destination-sensitive metadata or an external secret
exchange. Never commit cloud credentials to `forest.cue`.

## Mapping destinations in a project

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

| `FOREST_TERRAFORM_PLUGIN_CACHE_DIR` | Provider plugin cache shared by every run, so `init` installs a provider once per server process instead of on every plan and apply. Default `<tmp>/forest/terraform-plugin-cache`; an empty value turns it off. A run's lock file records the cached package's hash, and the deploy stage verifies against it. Nothing prunes old provider versions; the cache is cleared when the server's disk is. |

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

`forest.cue` declares which destinations an environment targets:

```cue
env: {
    dev: {
        destinations: [
            {destination: "forest-dev", type: "forage/containers@1"},
        ]
    }
    prod: {
        destinations: [
            {destination: "prod-ecs", type: "forest/generic@1"},
        ]
    }
}
```

Destination names can use the selector forms supported by release and trigger
configuration. The destination represents a place—account, region, cluster, or
runtime namespace—while project configuration supplies application-specific
values such as service or image identity.

## Releasing

Environment releases retain undeclared fan-out for non-generic destination types. External `forest/generic@1` destinations are opt-in: an artifact must contain a destination item for that environment, explicitly typed `forest/generic@1`, whose destination selector matches the name. Legacy or untyped items do not opt in. Naming a generic destination with `--destination` does not bypass this rule; naming a non-generic destination still overrides the project's selector. Creating a generic destination therefore reports no projects widened by implicit fan-out.

If the environment holds only generic destinations and the project has not opted in, the release fails rather than succeeding without deploying anywhere. The type and selector must be present in the rendered deployment item; adding a destination to Forest alone cannot enroll another project.

Target all eligible destinations in an environment:

```bash
forest release create --environment dev
```

Target an explicitly named destination:

```bash
forest release release --destination k8s-prod-eu
```

Route the environment through its configured pipeline:

```bash
forest release create --environment prod --pipeline
```

## Destination state

View the latest known release state:

```bash
forest project releases \
  --organisation acme \
  --project my-service
```

Forest records what was requested and the provider/runtime's reported outcome.
Production readiness also requires independent health signals and reconciliation
so a successful API call cannot be mistaken for a healthy rollout.

## CLI commands

```bash
forest destination create --organisation acme --name k8s-dev --environment dev --type forest/kubernetes@1
forest destination update --organisation acme --name k8s-dev
forest destination delete --organisation acme --name k8s-dev
forest destination list --organisation acme
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

