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
| `forest/kubernetes@1` | Kubernetes deployment via manifests |
| `forest/terraform@1` | Terraform apply/plan |
| `forage/containers@1` | Container orchestration |

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
