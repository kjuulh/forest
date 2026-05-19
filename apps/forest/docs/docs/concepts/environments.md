# Environments

Environments represent logical deployment stages — like `dev`, `staging`, and `prod`. They are the organisational layer between projects and destinations.

## Purpose

Environments let you:

- Define different configuration per stage (replicas, log levels, feature flags)
- Scope policies to specific stages (e.g., "require soak time before prod")
- Target releases at a stage level (`--environment prod`)
- Group destinations logically

## Creating Environments

```bash
forest environment create --organisation my-org --name dev
forest environment create --organisation my-org --name staging
forest environment create --organisation my-org --name prod
```

## Using Environments in Configuration

In `forest.cue`, component usage is organised by environment:

```cue
"forest-contrib": "kubernetes-service": sdk.#ForestComponentUsage & {
    env: {
        dev: {
            destinations: [{destination: "k8s-dev", type: "forest/kubernetes@1"}]
            config: {
                replicas: 1
                env_vars: [{key: "RUST_LOG", value: "debug"}]
            }
        }
        staging: {
            destinations: [{destination: "k8s-staging", type: "forest/kubernetes@1"}]
            config: {
                replicas: 2
                env_vars: [{key: "RUST_LOG", value: "info"}]
            }
        }
        prod: {
            destinations: [{destination: "k8s-prod", type: "forest/kubernetes@1"}]
            config: {
                replicas: 5
                env_vars: [{key: "RUST_LOG", value: "warn"}]
            }
        }
    }
}
```

Per-environment `config` merges with the shared `config` block — environment values take precedence.

## Environment in Policies

Policies reference environments to enforce guardrails:

- **Soak time**: "Must be deployed to `staging` for 2 hours before `prod`"
- **Branch restriction**: "Only `main` branch can be released to `prod`"
- **External approval**: "Require 2 approvals before releasing to `prod`"

See [Policies](policies.md) for details.

## CLI Commands

```bash
forest environment list --organisation my-org
forest environment create --organisation my-org --name dev
forest environment get --organisation my-org --name dev
forest environment update --organisation my-org --name dev
forest environment delete --organisation my-org --name dev
```
