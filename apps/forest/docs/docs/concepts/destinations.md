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

The exit status is `0` when in sync, `2` when something drifted (the refresh-only plan is printed), and `1` when the check couldn't run. So a scheduled CI job can alert on `2` without parsing anything.

The check runs synchronously in the request. Scheduling it and showing the result in forage are follow-ups.

