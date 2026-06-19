# forest-contrib/terraform-service

Deploy a Terraform-managed service. Implements the
`forest/deployment` hook contract; release pipelines that target a
`type: forest-contrib/terraform-service` destination flow through
these hooks.

## Spec

The component's `Spec` carries the canonical terraform-shaped fields:

- `name` — service name (used for the workspace + tag root).
- *(further fields will land as we wire real terraform actions: a
  module URL, var inputs, backend config, …)*

`validate` checks that `name` is non-empty.

## Hooks

- **`prepare`** — render variable files / select workspace. Today a
  stub; flesh out for real terraform-runner integration.
- **`release`** — `terraform apply` against the resolved workspace.
- **`status`** — read output state and decide healthy / unhealthy.

## Dependency

Imports `forest/deployment`. Coordinate version bumps with the hook
trait there.

## Publishing

```sh
cd apps/forest/components/forest-contrib/terraform-service
mise run forest -- components build
mise run forest -- components publish
```
