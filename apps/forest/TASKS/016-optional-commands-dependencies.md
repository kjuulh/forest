# 016: Make `#Commands` and `dependencies` optional in component specs

## Problem

Every component currently requires both `#Commands` and `dependencies` sections in `forest.component.cue` and `forest.cue`, even when they're empty:

```cue
// Required even when there are no commands:
#Commands: sdk.#ForestCommands & {}

// Required even when there are no dependencies:
dependencies: sdk.#ForestDependencies & {}

commands: sdk.#ForestCommands & {}
```

Resource-generator components (forage-postgresql, forage-nats, forage-s3) have no commands and only deployment hooks. Having to write empty `#Commands: sdk.#ForestCommands & {}` is boilerplate that adds confusion — it suggests commands are expected.

## Desired behavior

Both should be optional. When omitted:
- `#Commands` defaults to an empty command set
- `dependencies` defaults to no dependencies
- `commands` (in forest.cue) defaults to no project commands

The CUE schema definitions (`sdk.#ForestCommands`, `sdk.#ForestDependencies`) should be updated to allow omission.

## Files to change

- CUE SDK schema definitions (wherever `#ForestCommands`, `#ForestDependencies` are defined)
- `crates/forest/src/services/project.rs` — handle missing commands/dependencies gracefully
- `crates/forest-sdk-codegen/src/lower.rs` — handle missing commands schema (partially done in task 004)
- Component template scaffolding (`forest components init`) — only include sections when non-empty

## Testing

- Create a component with only hooks, no `#Commands` section
- Verify `forest generate`, `forest build`, `forest release prepare` all work
- Create a component with no dependencies section
- Verify it works as a standalone component
