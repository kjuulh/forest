# forest-contrib/ecs-service

Deploy an AWS ECS Fargate service. Implements the `forest/deployment`
hook contract, so any release pipeline whose destination is configured
with `type: forest-contrib/ecs-service` flows through these hooks.

## Spec

The component's `Spec` carries the canonical ECS-shaped fields:

- `name` — service name (and tag root).
- `image` — container image to deploy.
- `port` — port the container listens on.
- *(more fields will land here as the component grows: env vars, IAM
  role, target group, …)*

`validate` rejects empty / zero-port specs early.

## Hooks

- **`prepare`** — gather inputs and render the deployment manifests.
  Today this is a no-op stub (`manifests: []`); flesh out as we wire
  up the real ECS task-definition rendering.
- **`release`** — apply the change to the target AWS account.
- **`status`** — health-check the service.

## Dependency

Imports `forest/deployment` (relative path during local development,
registry once published). Bump both versions in lockstep when the
hook trait changes.

## Publishing

```sh
cd apps/forest/components/forest-contrib/ecs-service
mise run forest -- components build
mise run forest -- components publish
```
