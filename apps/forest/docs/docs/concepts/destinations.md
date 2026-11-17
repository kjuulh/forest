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

Generate one with `openssl rand -base64 32`.

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

